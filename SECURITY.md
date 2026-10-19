# Security policy

RustWTM's service runs as LocalSystem and can change or terminate processes,
so security issues are taken seriously.

## Reporting a vulnerability

Please **do not** open a public issue. Report privately through
[GitHub security advisories](https://github.com/ersinkoc/RustWTM/security/advisories/new)
or by email to ersinkoc@gmail.com. Include steps to reproduce and the
affected version. You should get a reply within a few days.

## Supported versions

Only the latest release receives fixes.

## Design notes

- Control requests over the named pipe are accepted only from elevated
  administrators; read-only requests are open to interactive users.
- Process detail queries are executed while impersonating the client.
- The pipe rejects remote clients, is created with
  `FILE_FLAG_FIRST_PIPE_INSTANCE` and always keeps an instance open, and
  clients verify that the server is the installed `RustWTM` service.
- The data directory (`%ProgramData%\RustWTM`, containing `config.toml` and
  the usage history) is writable only by SYSTEM and Administrators.
- Core Windows processes, critical processes and RustWTM itself can never be
  throttled, suspended or terminated.
