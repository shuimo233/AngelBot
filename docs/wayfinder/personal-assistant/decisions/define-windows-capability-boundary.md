# Define the Windows capability boundary

Status: closed
Type: research
Parent: [Personal Assistant Daily-Use Release](../MAP.md)
Research: [Windows daily-use capabilities](../../../research/windows-daily-use-capabilities.md)

## Question

Which Windows-native capabilities are required for the Daily-Use Release, and
where should platform-specific implementations end so the core remains compact
and testable?

## Resolution

The first fully supported environment is a current Windows 11 x64 interactive
user session. Windows behavior is isolated behind seven narrow capabilities:
protected credentials, application residency, notifications, file
authorization, external authentication, browser or desktop automation, and
installation/update/recovery.

Use opaque references to Windows-protected credentials, opt-in startup and tray
residency, actionable system notifications, explicit file/folder grants, and
system-browser PKCE for external account authorization. Browser work uses an
isolated profile. Desktop automation is limited to bounded, observable UI
Automation operations rather than presented as general RPA.

The supported release path is a signed per-user installer with signed updates,
recoverable local state, and an explicit restart/update experience. The release
does not promise execution while the machine is sleeping or no user is signed
in.

Windows services, elevation and UAC automation, machine-wide installation,
unbounded filesystem access, takeover of a user's default browser profile, and
general-purpose RPA are outside the boundary. Platform-specific code implements
these ports but must not leak Windows concepts into conversation, task,
workspace, or permission domain contracts.
