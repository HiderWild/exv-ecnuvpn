# Architecture fixtures

This folder contains **sanitized, deterministic** fixture artifacts used by tests and documentation.

## Hard requirements (never include)

Fixtures in this repository must **never** contain any of the following:

- real usernames or email addresses
- real passwords
- cookies or session tokens captured from a real login
- VPN host private addresses (or any internal address material that could identify an environment)
- device identifiers (hostname, device ID, serials, MAC addresses, IMEI, etc.)
- certificate material (PEM blocks, private keys, cert chains, fingerprints tied to a real deployment)

If you need to add new fixtures, redact first, then re-check the repository for accidental leakage.

## Redaction replacements (exact strings)

Use the following replacements **exactly** (case-sensitive):

- username: `student@example.invalid`
- password: `REDACTED_PASSWORD`
- cookie: `REDACTED_COOKIE`
- internal IPv4: `10.255.0.10`
- VPN server: `vpn.example.invalid`

Notes:
- Prefer RFC 5737 documentation IPv4 ranges (`192.0.2.0/24`, `198.51.100.0/24`, `203.0.113.0/24`) for example routes.
- Do not introduce additional redaction tokens; use the fixed names above.
