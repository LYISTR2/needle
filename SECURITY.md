# Security model

## Deliberate limits

Needle observes Linux hosts. It has no exec/terminal/file-transfer/update/plugin endpoint. The agent initiates HTTPS requests and never listens. A panel response is used only as an HTTP status; it cannot configure targets or run actions. Panel metadata is rendered with DOM textContent, not injected HTML. Theme values are validated color literals, not arbitrary CSS/JS.

The web dashboard is read-only. Enrollment, revocation and traffic edits require local OS administrator access. A 256-bit OS-random viewer token authorizes reads; separate 256-bit per-node tokens authorize reports for only that node. The database stores SHA-256 token digests. Comparison is constant-time for valid-format tokens. HTTPS is mandatory; agent redirects and environment proxies are disabled. Tokens are not cookies, query parameters or persistent browser storage.

Reports have strict field validation, monotonic persisted sequence numbers, per-node acceptance spacing, bounded bodies and concurrency. Revocation deletes the identity and its latest report. Monotonic sequence checks stop simple replay but are not protection against an adversary who steals the node token and invents higher sequences.

## Trust boundaries

- The reverse proxy terminates TLS. It sees requests and credentials and is trusted. It must replace forwarding headers, enforce connection/rate limits, and use a properly permissioned Unix socket.
- The backend has no AF_INET listener. Its service restricts socket families to AF_UNIX. Wrong Host, missing HTTPS proxy header and foreign Origin are rejected. Anyone with local socket access can forge these headers, so they are not independent cryptographic proof of TLS.
- The nginx/www-data group can connect to the socket but cannot read the panel database (state directory 0700). Do not put unrelated untrusted services in the same proxy group.
- The agent has no elevated capabilities. /proc metrics, statvfs and outbound TCP do not need root. Local target addresses are configured by the operator, never by the panel.
- systemd DynamicUser, NoNewPrivileges, ProtectSystem=strict, ProtectHome, PrivateTmp, PrivateDevices, protected kernel/control groups, empty capabilities, syscall filters, restricted address families, task/file limits and MemoryMax constrain both services. These are defense in depth, not a complete security boundary against kernel bugs.

## Compromise impact — no false guarantees

- A leaked viewer token exposes all monitored system metadata and traffic. It cannot use the supplied API to delete nodes or execute commands.
- A leaked node token can forge that node's telemetry and advance its sequence, disrupting authentic reports. It cannot read all nodes using the API or authenticate as another node.
- A compromised panel process can read/change/delete its database and read the viewer digest/config, alter web responses and steal the viewer token when it is entered. It can observe incoming agent bearer tokens. Do NOT claim panel compromise only allows deleting nodes.
- A malicious authenticated panel can withhold or reject reports. It has no intentional agent-control channel, but vulnerabilities in TLS/HTTP parsers and dependencies remain possible.
- A compromised agent process can read local information available to its unprivileged identity, read its credential, corrupt its ledger and initiate allowed-family outbound connections. There is no destination allowlist at the kernel level. For stronger egress isolation use administrator-managed network policy.
- Root, the kernel, reverse proxy, local privileged administrators and trusted build/dependency supply chains are outside this application boundary.

## Not provided

No independent security audit, penetration-test certification, fuzzing coverage guarantee, zero-loss billing, zero CVE claim, secure boot/attestation or signed release update infrastructure. Dependencies include bundled SQLite and Rust TLS/HTTP stacks. Cargo.lock pins versions for reproducibility, not proof of safety. Evaluate advisories and manually update dependencies before production and as new issues arise.

Avoid exposing the panel beyond necessary networks; use VPN/IP allowlists at the proxy where appropriate. Do not disable TLS verification, remove authentication, or weaken sandbox settings merely to make a deployment work.

## Credential recovery

Lost dashboard token cannot be recovered from its digest. Stop the panel and back up configuration. Run `needle init` against a new temporary config with the same origin/database/socket, safely save the newly printed token, merge any existing theme settings and replace the old config atomically as root, then restart and verify both the new token and rejection of the old token. Do not overwrite the database.

For an exposed agent token, revoke the node locally, register it again, distribute the new configuration through a protected channel, and preserve the ledger/sequence if identity is unchanged. For an exposed viewer token rotate it as above. Remove leaked copies and review proxy/host access.

Report vulnerabilities privately to the deployment operator/repository maintainer. Never include live credentials in a public report.
