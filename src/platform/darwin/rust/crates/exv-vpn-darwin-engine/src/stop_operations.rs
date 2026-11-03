//! 停止操作的本地内存回执；不持资源、不另建持久库。
use exv_vpn_wire::generated as wire;
use std::{collections::HashMap, future::Future};
use tokio::sync::{Mutex, watch};
use tonic::Status;

type StopKey = (Vec<u8>, i32, Vec<u8>, Vec<u8>);
fn operation_key(key: &wire::OperationLookupKey) -> StopKey {
    (
        key.principal_digest.clone(),
        key.method,
        key.runtime_epoch.clone(),
        key.operation_id.clone(),
    )
}

type Completion = watch::Receiver<Option<wire::StopTunnelReply>>;
#[derive(Default)]
pub(crate) struct StopOperations {
    entries: Mutex<HashMap<StopKey, (Vec<u8>, Completion)>>,
}
impl StopOperations {
    pub(crate) async fn run<F, Fut>(
        &self,
        key: wire::OperationLookupKey,
        digest: Vec<u8>,
        execute: F,
    ) -> Result<wire::StopTunnelReply, Status>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = wire::StopTunnelReply> + Send + 'static,
    {
        let id = operation_key(&key);
        let mut entries = self.entries.lock().await;
        let mut completion = if let Some((original, reply)) = entries.get(&id) {
            if original != &digest {
                return Err(Status::already_exists("DARWIN_STOP_DIGEST_CONFLICT"));
            }
            reply.clone()
        } else {
            let (send, receive) = watch::channel(None);
            entries.insert(id, (digest, receive.clone()));
            // unary被取消不撤销已受理停止；真实执行持有自己的生命周期许可至回执。
            tokio::spawn(async move {
                let task = tokio::spawn(execute());
                let reply = task.await.unwrap_or_else(|_| {
                    stop_reply(false, false, vec![stop_error("darwin/stop_task", None)])
                });
                send.send_replace(Some(reply));
            });
            receive
        };
        drop(entries);
        loop {
            if let Some(reply) = completion.borrow().clone() {
                return Ok(reply);
            }
            completion
                .changed()
                .await
                .map_err(|_| Status::internal("DARWIN_STOP_COMPLETION_LOST"))?;
        }
    }
    pub(crate) async fn get(&self, key: &wire::OperationLookupKey) -> wire::GetOperationReply {
        let entries = self.entries.lock().await;
        let state = match entries.get(&operation_key(key)) {
            Some((_, reply)) => match reply.borrow().as_ref() {
                Some(reply) => wire::operation_state::State::Terminal(terminal(reply)),
                None => wire::operation_state::State::Pending(wire::EmptyOperationState {}),
            },
            None => wire::operation_state::State::Unknown(wire::EmptyOperationState {}),
        };
        wire::GetOperationReply {
            state: Some(wire::OperationState { state: Some(state) }),
        }
    }
}
pub(crate) fn stop_error(reason: &str, errno: Option<i32>) -> wire::VpnError {
    wire::VpnError {
        code: wire::ErrorCode::ObservedConflict as i32,
        stage: wire::ErrorStage::Teardown as i32,
        retry: wire::RetryAdvice::UseNewOperation as i32,
        reason: reason.into(),
        native: errno.map(|errno| wire::RedactedNativeError {
            category: wire::NativeErrorCategory::Resource as i32,
            namespace: wire::NativeErrorNamespace::PosixErrno as i32,
            code: i64::from(errno),
        }),
        ..Default::default()
    }
}
pub(crate) fn stop_reply(
    stopped: bool,
    blocks_new_connect: bool,
    errors: Vec<wire::VpnError>,
) -> wire::StopTunnelReply {
    let completion = wire::StopCompletion {
        stopped,
        blocks_new_connect,
        errors,
    };
    let result = if completion.errors.is_empty() {
        wire::stop_tunnel_reply::Result::Stopped(wire::MutationReceipt::default())
    } else {
        wire::stop_tunnel_reply::Result::Failed(wire::OperationFailed {
            receipt: None,
            error: completion.errors.first().cloned(),
        })
    };
    wire::StopTunnelReply {
        stop_completion: Some(completion),
        result: Some(result),
    }
}
fn terminal(reply: &wire::StopTunnelReply) -> wire::OperationTerminal {
    let result = match reply.result.as_ref() {
        Some(wire::stop_tunnel_reply::Result::Stopped(receipt)) => {
            Some(wire::operation_terminal::Result::Succeeded(receipt.clone()))
        }
        Some(wire::stop_tunnel_reply::Result::Failed(failed)) => {
            Some(wire::operation_terminal::Result::Failed(failed.clone()))
        }
        _ => None,
    };
    wire::OperationTerminal {
        stop_completion: reply.stop_completion.clone(),
        result,
    }
}
