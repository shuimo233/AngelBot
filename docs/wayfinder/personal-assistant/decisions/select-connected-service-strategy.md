# Select the email and calendar connection strategy

Status: closed
Type: research
Parent: [Personal Assistant Daily-Use Release](../MAP.md)
Research: [Connected services on Windows](../../../research/connected-services-windows.md)

## Question

Which standards, provider APIs, and Windows authentication flows let an
open-source desktop application support practical email and calendar workflows
without an AngelBot account, hosted secret, or provider lock-in?

## Resolution

Expose stable mail and calendar capabilities through Service Connectors. Use
provider-native delegated APIs as the default for Google and Microsoft, with
IMAP, SMTP, and CalDAV as compatibility fallbacks rather than an alternate
permission model.

Desktop authorization uses the system browser, authorization code flow, PKCE,
and a loopback callback. AngelBot never ships or requests a confidential client
secret. Tokens remain in protected local credential storage; background work
must pause for reconnection when silent renewal is no longer possible.

Use polling and provider delta mechanisms in the first Windows release rather
than requiring a hosted webhook. Request scopes in explicit capability stages,
separating reading, drafting, sending, and calendar mutation wherever the
provider permits it.

Microsoft Graph delegated mail and calendar permissions are the lowest-risk
candidate for the first complete account workflow. Google Calendar and
send-only Gmail can follow the same boundary. Reading or organizing Gmail is a
separate release gate because the required Restricted scopes, provider
verification, and transmission of mail content to a remote model can require a
security assessment and additional disclosure. IMAP/SMTP does not bypass this
gate because Google requires the broader restricted `mail.google.com` scope.

The default consumer experience may use public desktop client identifiers owned
by the project, while advanced and managed environments may supply their own
application registration. This does not create an AngelBot account or hosted
control plane.
