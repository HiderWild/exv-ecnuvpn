# macOS Native Route Crash Cleanup

This is a manual emergency and validation procedure for native macOS route
configuration after an app/helper crash leaves stale routes behind. It is not
the normal native-mode path: a healthy native session must create and clean up
its owned routes through the native route configuration layer, without shell
network configuration.

Use this procedure only on a validation machine, or when an operator has already
confirmed that no active ECNU-VPN native session is managing the routes.

## Warnings And Safety Rules

- Do not delete routes while a VPN session is still connected or reconnecting.
- Delete only routes that match the captured ECNU-VPN session evidence: VPN
  server IP, ECNU split route destination, utun interface, upstream interface,
  and gateway.
- Do not delete `default`, `0.0.0.0/0`, local LAN, multicast, loopback,
  non-ECNU, or unrelated user/system routes.
- Do not broaden a command after a failed delete. Re-check the route evidence
  and stop if the route no longer matches the stale native session.
- Prefer scoped deletes with `-ifscope <interface>` when the route table or
  `route -n get` shows an interface scope.

## Capture Before Evidence

Record these commands before changing anything:

```bash
date
pgrep -fl "ECNU|exv|exv-helper|openconnect" || true
ifconfig | grep -A8 '^utun' || true
netstat -rn -f inet
scutil --nwi
```

Also capture the session values that identify owned routes:

- VPN server hostname and resolved IPv4 address.
- Expected ECNU split routes from the app configuration, native session logs, or
  CSTP split-include metadata.
- The utun name and interface index reported by native session state or logs.
- The pre-tunnel upstream interface and gateway used for the VPN server bypass
  route.

For each candidate destination, capture an exact route lookup:

```bash
route -n get <vpn-server-ip>
route -n get <ecnu-split-route-probe-ip>
netstat -rn -f inet | grep -E '(^|[[:space:]])(<vpn-server-ip>|<ecnu-network>)'
```

Use a representative in-range probe IP for a split network when `route -n get`
does not accept the CIDR directly.

## Identify Owned Routes

### VPN Server Bypass Route

The server bypass route keeps the control connection outside the tunnel. It is
owned by the native session only when all of these are true:

- Destination is the resolved VPN server IPv4 address, usually a `/32` host
  route.
- Gateway and interface match the pre-tunnel upstream route captured before
  tunnel setup, such as `en0` plus the LAN gateway.
- The route is not through the ECNU `utun` interface.
- If `route -n get <vpn-server-ip>` reports an `ifscope`, it matches the
  upstream interface, not another user-selected interface.

Typical evidence looks like:

```text
route to: <vpn-server-ip>
gateway: <upstream-gateway>
interface: en0
```

### ECNU Split Routes

ECNU split routes send campus networks through the tunnel. A split route is
owned by the native session only when all of these are true:

- Destination matches an ECNU split include or configured ECNU route from the
  same session, for example `<ecnu-network>/<prefix>`.
- `netstat -rn -f inet` shows `Netif` as the session `utun` interface.
- `route -n get <probe-ip-inside-route>` shows `interface: utunN`.
- The route is not the system default route and does not point at the upstream
  LAN interface.

Scoped routes are expected for native macOS route configuration. In
`netstat -rn -f inet`, look for the exact destination with the `utunN` netif. In
`route -n get`, look for `interface: utunN` and, when present, `ifscope: utunN`.

## Safe Cleanup Procedure

Replace every placeholder with values captured above. Run one delete at a time,
then immediately re-check that exact route.

1. Confirm there is no active native VPN session managing routes.

```bash
pgrep -fl "ECNU|exv|exv-helper" || true
```

If the helper is installed and still running as a service, confirm from the app
or logs that it is not holding an active tunnel session before deleting routes.

2. Delete stale ECNU split routes through the stale utun interface.

For a network route:

```bash
sudo route -n delete -net <ecnu-network> -netmask <ecnu-netmask> -ifscope <utunN>
route -n get <probe-ip-inside-route> || true
```

For a `/32` split route:

```bash
sudo route -n delete -host <ecnu-host-ip> -ifscope <utunN>
route -n get <ecnu-host-ip> || true
```

If the route is visible on `utunN` but not scoped, use the exact non-scoped
delete only after re-capturing `netstat -rn -f inet` and `route -n get` evidence:

```bash
sudo route -n delete -net <ecnu-network> -netmask <ecnu-netmask> -interface <utunN>
```

3. Delete the stale VPN server bypass host route.

Use this only when the destination is the VPN server IPv4 address and the route
still points at the captured upstream gateway/interface:

```bash
sudo route -n delete -host <vpn-server-ip> -ifscope <upstream-if>
route -n get <vpn-server-ip>
```

If the bypass route is not scoped, delete only the exact host route after
verifying the gateway and interface still match the stale session:

```bash
sudo route -n delete -host <vpn-server-ip>
route -n get <vpn-server-ip>
```

Do not delete a current server route if `route -n get <vpn-server-ip>` now points
to a different gateway/interface than the stale-session evidence.

## Capture After Evidence

Record the same evidence again:

```bash
date
pgrep -fl "ECNU|exv|exv-helper|openconnect" || true
ifconfig | grep -A8 '^utun' || true
netstat -rn -f inet
scutil --nwi
route -n get <vpn-server-ip>
route -n get <probe-ip-inside-each-ecnu-route> || true
```

Validation passes when:

- The stale ECNU split routes no longer resolve through the old `utunN`.
- The stale VPN server bypass host route is gone, or has been replaced by the
  normal system route through the current upstream interface.
- Unrelated user/system routes, especially the default route and local LAN
  routes, are unchanged.
- No shell command was needed for the normal native connect/disconnect path;
  these `route -n delete` commands were used only for manual crash cleanup or
  validation.
