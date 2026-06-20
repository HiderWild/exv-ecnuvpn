# Modal Onboarding and Config Initialization Design

## Context

The desktop client currently has several independent modal implementations:
in-window Vue `Teleport` dialogs, a dedicated native modal route, service
install overlays, and core crash overlays. Their spacing, scrim behavior, and
focus patterns drift. The current first-run service install prompt is also
renderer-owned through `localStorage` and `service_install_prompt_seen`, while
the desired behavior is core-owned first-run detection based on persisted
configuration validity.

The settings page also has unreadable native select options in the dark theme.
The connect flow prompts only for a password even when username is missing.

This design chooses the unified compact modal shell approach.

## Requirements

- All modal scrims must be clipped to the visible content contour inside the
  app window border. The transparent shadow gutter must remain unmasked.
- All modal dialogs must use one compact visual system with smaller margins and
  less empty padding.
- Core, not the renderer, decides whether first-run quick start should be
  shown.
- Missing or invalid config files are treated as first run even if the helper
  service is already installed.
- Core must replace missing or invalid config with a complete minimal config
  before the renderer asks for settings.
- Quick start can be skipped. Skipping writes nothing and does not install the
  service, because core has already written defaults; it must not show again on
  the next launch.
- Quick mode asks only for username and password. It uses
  `vpn-ct.ecnu.edu.cn` as the default server, defaults to remember password,
  and defaults to installing the helper service.
- Custom mode adds routes, DTLS, and MTU.
- Both quick and custom modes include an import config action.
- Connect-time credential completion must prompt for every missing required
  credential, not only the password.
- Username entered during credential completion is always persisted.
- The credential completion password has a remember-password checkbox. If the
  user checks it and confirms, it is equivalent to enabling remember password
  in settings and saving the encrypted password.
- Settings native selects must have readable option foreground/background
  colors.

## Architecture

### Shared Modal Foundation

Add a reusable renderer modal foundation for in-window dialogs. It owns:

- content-contour scrim placement,
- compact panel sizing and padding,
- focus management and escape routes,
- title, description, icon, action row, and field error layout,
- consistent z-index layering.

`AppWindowFrame` remains the owner of the physical window shadow gutter and
border. The modal foundation renders inside the content surface, not directly
against the full viewport, so the scrim never covers the shadow margin.

The dedicated native modal route cannot literally share DOM with the main
window, but it must reuse the same spacing, radius, type scale, color tokens,
and action layout. The route should be treated as another consumer of the same
modal design tokens.

Consumers include:

- error dialog,
- confirm dialog,
- auth continuation dialog,
- password/import password prompt,
- close app prompt,
- first-run quick start,
- service install loading overlay,
- core crashed overlay,
- existing service install modal route until it is superseded by quick start.

### Core-Owned First-Run Decision

Core performs config initialization during startup after runtime paths are
bootstrapped and before normal config read actions are served.

The startup check returns:

- `normal`: config file exists, parses, and contains all required initialized
  fields with valid JSON types.
- `missing`: config file does not exist.
- `invalid`: config file exists but cannot parse or does not contain the
  complete initialized field set.

For `missing` or `invalid`, core removes or overwrites the existing config and
writes a new minimal default config. It then emits one renderer event for the
current frontend session:

```json
{
  "type": "quick-start-request",
  "data": {
    "reason": "missing",
    "defaults": {
      "server": "vpn-ct.ecnu.edu.cn",
      "remember_password": true,
      "install_service": true
    }
  }
}
```

`reason` is `missing` or `invalid`. The event is not persisted as a frontend
flag. If the user skips quick start, no data is written. Because core has
already written a complete default config, the next launch is `normal` and does
not emit the event.

The minimal required initialized fields are the persisted fields currently in
`Config`:

- `server`
- `username`
- `password`
- `mtu`
- `useragent`
- `disable_dtls`
- `remember_password`
- `routes`
- `extra_args`
- `log_file`
- `vpn_engine`
- `windows_tunnel_driver`
- `windows_tap_interface`
- `auto_reconnect`
- `minimal_mode`
- `service_install_prompt_seen`
- `minimal_install_service_before_connect`

Username and password may be empty in a valid initialized config. Completeness
means the field exists and has the expected type, not that user credentials
have been supplied.

## Quick Start UI

Quick start opens when the renderer receives `quick-start-request`.

The panel has:

- compact header with concise title and description,
- segmented control: `Quick` and `Custom`,
- import config action available in both modes,
- primary action, skip action, and busy state.

Quick mode fields:

- username,
- password.

Quick mode implicit defaults:

- server: `vpn-ct.ecnu.edu.cn`, matching the settings dropdown host value
  without a URL scheme,
- remember password: true,
- install service: true.

Custom mode fields:

- username,
- password,
- routes,
- DTLS,
- MTU.

Routes use the existing WebUI token input pattern so each CIDR is a discrete
editable item. DTLS is a switch. MTU is a numeric input with existing valid
range behavior.

Confirm behavior:

1. Validate visible fields.
2. Save auth config with the selected server, username, password, and
   `remember_password=true`.
3. Save custom settings if custom mode is active.
4. If service is not installed and the mode requests installation, invoke the
   existing service install action.
5. Close the dialog on successful config save. Service installation failure is
   shown as a recoverable error and does not roll back saved config.

Import config behavior:

- Opens the existing import flow.
- If import succeeds, refresh auth/settings and close quick start.
- If import fails, keep quick start open and show the error through the unified
  modal error presentation.

Skip behavior:

- Close quick start.
- Do not save auth, settings, service prompt state, or any renderer-local flag.
- Do not install service.

## Connect-Time Credential Completion

Before `vpn.connect` or elevated connect sends a request, the renderer refreshes
auth config and computes missing fields:

- username is missing if `auth.username.trim()` is empty.
- password is missing if neither a stored password is available nor a one-shot
  password has been supplied.

The credential completion dialog displays only missing fields:

- missing username only: username field,
- missing password only: password field with remember checkbox,
- both missing: username and password fields.

Validation is field-local. Confirm is blocked until required visible fields are
non-empty. The first invalid field receives focus.

Submit behavior:

- Username is persisted immediately through auth config before connecting.
- If remember password is checked, save auth with
  `remember_password=true` and the password, then connect without passing a
  plaintext password in the request body.
- If remember password is unchecked, persist username only and pass the password
  as the one-shot connect argument.
- If saving username or remembered password fails, do not connect.
- If the user cancels, do not connect.

The remember checkbox default mirrors the current auth setting. Checking it in
this dialog is equivalent to enabling remember password in settings.

## Modal Visual Rules

All dialogs use the compact modal scale:

- panel max width: 400 px by default, 440 px for quick start custom mode,
- panel padding: 16 to 20 px,
- title size: 14 to 16 px,
- body copy: 12 to 14 px,
- button height: at least 36 px,
- radius: no larger than the existing 8 px style unless the current component
  already requires otherwise,
- spacing follows the existing 4/8 px rhythm,
- one primary action per modal,
- secondary and destructive actions are visually subordinate or separated.

The scrim should be strong enough for modal legibility in dark mode, but it
must be scoped to the content surface. It must not blur or tint the window
shadow.

Native select dropdowns receive explicit dark option colors, for example a
surface/background token for `option` and foreground token for text, with hover
and selected states kept readable where the platform permits styling.

## Error Handling

- Config initialization parse errors are logged, the invalid file is replaced,
  and quick start receives `reason="invalid"`.
- Quick start field validation errors are shown near fields.
- Quick start save failure keeps the dialog open.
- Service install failure after quick start config save does not undo saved
  credentials/settings.
- Import failure keeps quick start open.
- Credential completion never silently downgrades a failed remembered-password
  save into a temporary password connect.
- Existing `user_cancelled` handling remains non-error UI.

## Tests

Implementation must follow TDD.

C++ tests:

- missing config writes a complete default config and reports first-run repair,
- invalid config is replaced and reports first-run repair,
- complete config does not report first-run repair,
- completeness requires all initialized fields with correct JSON types,
- contract includes `quick-start-request` event type,
- config read actions after repair return complete auth/settings payloads.

WebUI host/type tests:

- generated event type includes `quick-start-request`,
- event payload type is available to renderer code,
- old frontend-local service prompt ownership assertions are replaced with
  core-owned quick-start assertions.

Renderer tests:

- quick start opens only on the event,
- quick skip closes without saving config or installing service,
- quick confirm saves username/password with remember password and default
  server,
- custom confirm saves routes, DTLS, and MTU,
- import success closes quick start and refreshes config,
- missing username/password combinations render the correct credential fields,
- remember password checkbox saves password and setting together,
- unchecked password is one-shot only,
- all in-window modal components use the shared modal foundation or shared
  compact modal tokens,
- settings select option readability styles exist.

Verification commands:

- `pnpm --dir webui test:host`
- `pnpm --dir webui exec vue-tsc -b`
- focused C++ targets for config initialization, contract, and app API
- browser visual verification for scrim clipping, select readability, and quick
  start mode switching when available

## Out of Scope

- Changing VPN protocol behavior.
- Changing helper service installation semantics beyond invoking the existing
  action from quick start.
- Adding new persistent frontend first-run flags.
- Redesigning the full settings page layout beyond select readability.
