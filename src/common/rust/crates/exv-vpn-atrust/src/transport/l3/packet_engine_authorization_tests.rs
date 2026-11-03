//! S6 授权解析次数与匹配边界；只运行局部 PacketEngine，不联网。
use super::*;
use crate::resource::ResourceParseOptions;
use crate::transport::l3::flow_auth::response_json_calls;
use serde_json::{Value, json};

fn engine() -> PacketEngine {
    let catalog = ResourceCatalog::from_json(
        br#"{"Data":{"AppList":{"Data":{"AppInfo":[{"Apps":[{"ID":"campus","AccessModel":"L3VPN","EnableTCPPrefL3":true,"NodeGroupID":"g","AddressList":[{"Host":"10.0.0.2","Protocol":"all","Port":"1-65535"}]}]}]}}}}"#,
        &ResourceParseOptions::new(),
    ).unwrap();
    PacketEngine::new(
        "10.0.0.1".parse().unwrap(),
        "g".into(),
        catalog,
        FlowIdentity {
            sid: Secret::new("fixture-sid"),
            sign_key: Secret::new("00ff"),
            device_id: "fixture-device".into(),
            connection_id: "fixture-connection".into(),
            process_hash: "fixture-hash".into(),
            environment: None,
        },
        FlowLimits::default(),
    )
}

fn udp(port: u16) -> Vec<u8> {
    let mut packet = vec![0; 29];
    packet[0] = 0x45;
    packet[2..4].copy_from_slice(&29u16.to_be_bytes());
    packet[8] = 64;
    packet[9] = 17;
    packet[12..16].copy_from_slice(&[10, 0, 0, 1]);
    packet[16..20].copy_from_slice(&[10, 0, 0, 2]);
    let sum = packet[..20]
        .chunks_exact(2)
        .map(|w| u32::from(u16::from_be_bytes([w[0], w[1]])))
        .sum::<u32>();
    packet[10..12].copy_from_slice(&(!(sum as u16)).to_be_bytes());
    packet[20..22].copy_from_slice(&port.to_be_bytes());
    packet[22..24].copy_from_slice(&53u16.to_be_bytes());
    packet[24..26].copy_from_slice(&9u16.to_be_bytes());
    packet[28] = 1;
    packet
}

fn start(engine: &mut PacketEngine, port: u16, now: Instant) -> u64 {
    let frames = engine.submit(udp(port), now).unwrap();
    let [L3Frame::AuthRequest { payload }] = &frames[..] else {
        panic!("authorization request")
    };
    serde_json::from_slice::<Value>(payload).unwrap()["conntrackHash"]
        .as_u64()
        .unwrap()
}

fn reply(status: u8, body: Value) -> L3Frame {
    L3Frame::AuthResponse {
        status,
        payload: serde_json::to_vec(&body).unwrap(),
    }
}

fn grant(id: u64) -> L3Frame {
    reply(
        0,
        json!({"code":0,"data":{"conntrackHash":id,"connectToken":"fixture-token"}}),
    )
}

fn ip(port: u16) -> Value {
    json!({"atype":2048,"protocol":17,"srcAddr":"10.0.0.1","srcPort":port,"destAddr":"10.0.0.2","destPort":53})
}

fn empty(actions: &PacketActions) {
    assert!(actions.to_node.is_empty());
    assert!(actions.to_host.is_empty());
    assert!(actions.rejected.is_empty());
    assert!(actions.dropped.is_empty());
}

// 若解析仍放在逐 Pending 匹配内部，未知 ID 的一次响应会重复解析 N 次。
#[test]
fn unknown_id_decodes_once_and_keeps_every_pending_flow() {
    let now = Instant::now();
    let mut engine = engine();
    let ids: Vec<_> = (1000..1064)
        .map(|port| start(&mut engine, port, now))
        .collect();
    response_json_calls();
    empty(&engine.receive(reply(0x82, json!({"code":99,"data":{"conntrackHash":99999,"connectToken":0,"ip":ip(1000)}})), now).unwrap());
    assert_eq!(engine.pending_bytes(), 64 * 29);
    let calls = response_json_calls();
    for id in ids {
        assert_eq!(engine.receive(grant(id), now).unwrap().to_node.len(), 1);
    }
    assert_eq!(engine.pending_bytes(), 0);
    assert_eq!(calls, 1, "unknown ID must not decode once per pending flow");
}

// 计数来自实际 serde 入口；规模/成功数也由真实引擎动作检查。
#[test]
fn scale_decodes_each_grant_or_rejection_once_and_hot_packets_never_decode() {
    let mut mismatches = Vec::new();
    for requested in [1, 64, 1024, 4096] {
        for granted in [true, false] {
            let now = Instant::now();
            let mut engine = engine();
            let ids: Vec<_> = (0..requested)
                .map(|i| start(&mut engine, 10000 + i as u16, now))
                .collect();
            assert_eq!(
                ids.iter().collect::<std::collections::HashSet<_>>().len(),
                requested
            );
            assert_eq!(engine.pending_bytes(), requested * 29);
            response_json_calls();
            let started = Instant::now();
            for id in ids {
                let actions = engine
                    .receive(
                        if granted {
                            grant(id)
                        } else {
                            reply(
                                0x82,
                                json!({"code":1,"data":{"conntrackHash":id,"connectToken":0}}),
                            )
                        },
                        now,
                    )
                    .unwrap();
                assert_eq!(actions.to_node.len(), usize::from(granted));
                assert_eq!(actions.rejected.len(), usize::from(!granted));
            }
            let elapsed_us = started.elapsed().as_micros();
            let calls = response_json_calls();
            assert_eq!(engine.pending_bytes(), 0);
            if granted {
                for i in 0..requested {
                    assert!(matches!(
                        &engine.submit(udp(10000 + i as u16), now).unwrap()[..],
                        [L3Frame::DataRequest(_)]
                    ));
                }
                assert_eq!(response_json_calls(), 0);
            }
            println!(
                "{}",
                json!({"experiment":"authorization_decode_count","evidence":"local_packet_engine_test_instrumentation","requested_flows":requested,"actual_created_flows":requested,"outcome":if granted {"granted"} else {"rejected"},"json_parse_calls":calls,"authorization_batch_us":elapsed_us,"timing_kind":"debug_test_instrumented_wall_clock_single_batch_includes_response_construction"})
            );
            if calls != requested {
                mismatches.push((requested, granted, calls));
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "responses decoded repeatedly: {mismatches:?}"
    );
}

// 将 decode 提前到空 Pending 判断之前，会让这里从空动作变成错误。
#[test]
fn no_pending_ignores_malformed_or_oversized_responses_without_decoding() {
    let now = Instant::now();
    let mut engine = engine();
    for ready in [false, true] {
        if ready {
            let id = start(&mut engine, 1000, now);
            engine.receive(grant(id), now).unwrap();
        }
        for payload in [
            b"{".to_vec(),
            vec![b' '; 65536],
            br#"{"code":"wrong","data":null}"#.to_vec(),
        ] {
            response_json_calls();
            empty(
                &engine
                    .receive(L3Frame::AuthResponse { status: 0, payload }, now)
                    .unwrap(),
            );
            assert_eq!(response_json_calls(), 0);
        }
    }
}

// 非零 ID 优先于冲突五元组；ID 缺失/0 才完整回退。
#[test]
fn nonzero_id_does_not_fall_back_and_zero_or_missing_id_matches_full_tuple() {
    let now = Instant::now();
    for id_field in [None, Some(0)] {
        let mut engine = engine();
        start(&mut engine, 1000, now);
        let second = start(&mut engine, 2000, now);
        let mut body = json!({"code":0,"data":{"ip":ip(1000),"connectToken":"by-tuple"}});
        if let Some(id) = id_field {
            body["data"]["conntrackHash"] = json!(id);
        }
        let actions = engine.receive(reply(0, body), now).unwrap();
        let [L3Frame::DataRequest(data)] = &actions.to_node[..] else {
            panic!("tuple grant")
        };
        assert_eq!(data.token, b"by-tuple");
        assert_eq!(&data.packets[0][20..22], &1000u16.to_be_bytes());
        assert_eq!(engine.pending_bytes(), 29);
        assert_eq!(engine.receive(grant(second), now).unwrap().to_node.len(), 1);
    }
    let mut engine = engine();
    let first = start(&mut engine, 1000, now);
    let second = start(&mut engine, 2000, now);
    let actions = engine.receive(reply(0,json!({"code":0,"data":{"conntrackHash":second,"ip":ip(1000),"connectToken":"by-id"}})),now).unwrap();
    let [L3Frame::DataRequest(data)] = &actions.to_node[..] else {
        panic!("ID grant")
    };
    assert_eq!(&data.packets[0][20..22], &2000u16.to_be_bytes());
    assert_eq!(engine.receive(grant(first), now).unwrap().to_node.len(), 1);
}

// 完整五元组任一字段不同都不得回退命中。
#[test]
fn fallback_requires_every_tuple_field_and_ipv4_address_type() {
    let now = Instant::now();
    for (field, value) in [
        ("atype", json!(1)),
        ("protocol", json!(6)),
        ("srcAddr", json!("10.0.0.3")),
        ("srcPort", json!(1001)),
        ("destAddr", json!("10.0.0.3")),
        ("destPort", json!(54)),
    ] {
        let mut engine = engine();
        let id = start(&mut engine, 1000, now);
        let mut tuple = ip(1000);
        tuple[field] = value;
        empty(
            &engine
                .receive(
                    reply(
                        0x82,
                        json!({"code":99,"data":{"conntrackHash":0,"ip":tuple,"connectToken":0}}),
                    ),
                    now,
                )
                .unwrap(),
        );
        assert_eq!(engine.pending_bytes(), 29);
        assert_eq!(engine.receive(grant(id), now).unwrap().to_node.len(), 1);
    }
}

// 形状错误仍是整个响应错误，不删除遍历到的流；诊断回退仍是第二次 JSON 解析。
#[test]
fn malformed_response_keeps_shape_categories_and_pending_budget() {
    let cases = [
        (b"{".to_vec(), FlowAuthError::InvalidJson, 2),
        (br#"{"code":"wrong","data":{"conntrackHash":99999,"connectToken":0}}"#.to_vec(), FlowAuthError::InvalidResponseShape {code:"string",data:"object",hash:"number",token:"number",ip:"missing"},2),
        (br#"{"code":0,"data":{"conntrackHash":99999,"connectToken":"secret","ip":{"atype":2048}}}"#.to_vec(), FlowAuthError::InvalidResponseShape {code:"number",data:"object",hash:"number",token:"string",ip:"object"},2),
        (vec![b' ';65536],FlowAuthError::PayloadTooLarge,0),
    ];
    let now = Instant::now();
    for (payload, expected, calls) in cases {
        let mut engine = engine();
        let first = start(&mut engine, 1000, now);
        let second = start(&mut engine, 2000, now);
        response_json_calls();
        let Err(PacketError::Authorization(error)) =
            engine.receive(L3Frame::AuthResponse { status: 0, payload }, now)
        else {
            panic!("response error")
        };
        assert_eq!(error, expected);
        assert_eq!(response_json_calls(), calls);
        assert_eq!(engine.pending_bytes(), 58);
        assert_eq!(engine.receive(grant(first), now).unwrap().to_node.len(), 1);
        assert_eq!(engine.receive(grant(second), now).unwrap().to_node.len(), 1);
    }
}

// 匹配之后保持 status→code→token 的业务优先级。
#[test]
fn matched_rejection_uses_status_then_code_then_token() {
    let now = Instant::now();
    for (status, code, token, expected) in [
        (
            0x82,
            99,
            json!(0),
            FlowAuthError::StatusWithCode {
                status: 0x82,
                code: 99,
            },
        ),
        (0x82, 0, json!(0), FlowAuthError::Status(0x82)),
        (0, 99, json!(0), FlowAuthError::Rejected(99)),
        (0, 0, json!(0), FlowAuthError::InvalidToken),
        (0, 0, json!(""), FlowAuthError::InvalidToken),
        (0, 0, json!("x".repeat(256)), FlowAuthError::InvalidToken),
    ] {
        let mut engine = engine();
        let id = start(&mut engine, 1000, now);
        let actions = engine
            .receive(
                reply(
                    status,
                    json!({"code":code,"data":{"conntrackHash":id,"connectToken":token}}),
                ),
                now,
            )
            .unwrap();
        assert!(actions.to_node.is_empty());
        let [
            FlowRejection {
                reason: FlowFailure::Authorization(error),
                ..
            },
        ] = &actions.rejected[..]
        else {
            panic!("matched flow rejection")
        };
        assert_eq!(*error, expected);
        assert_eq!(engine.pending_bytes(), 0);
    }
}
