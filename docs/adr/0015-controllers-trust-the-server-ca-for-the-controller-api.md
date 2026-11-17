# Controllers trust the Server's CA for the Controller API

A Server installed without a domain name has no certificate a public CA would issue without holding port 80 or 443, and the person installing it may need those ports for something else. In that install the Server issues a certificate for its own IP address from its CA, and the Dashboard and the Controller API are served over HTTPS with it on another port. Every install keeps the CA certificate it received at registration, checked against the fingerprint in the Server Key, and accepts it for the Controller API alongside the public CAs. So `rcli` and `rcli mcp` get an encrypted, authenticated connection to a Server that has only an IP address.

## Considered Options

- Plain HTTP on the IP address: sends Dashboard passwords and sessions in the clear. Rejected.
- A Let's Encrypt certificate for the IP address: lasts six days and needs port 80 or 443 at every renewal.
- A free hostname from a third-party DNS service: makes every such Server depend on that service.

## Consequences

- A browser does not know the Server's CA, so the Dashboard on an IP address shows a certificate warning until the person imports the CA.
- The install command for a new Device cannot rely on the system's certificates: on Linux and macOS it carries the CA certificate, on Windows the binary is downloaded through the browser.
- Losing the CA now also breaks Controller access on such a Server, not only Agent connections.
