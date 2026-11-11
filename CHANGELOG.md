# Changelog

## Unreleased

### Security

- Default listen address is `127.0.0.1`; Docker images still bind `0.0.0.0` and compose publishes the panel to `127.0.0.1:8080`.
- First administrator requires a one-time init token and captcha.
- Passwords use argon2id (legacy unsalted MD5 still verifies and is upgraded on login).
- Session tokens, captchas, and htpasswd salts use the OS CSPRNG.
- Login and captcha are rate-limited; secret settings are masked; `conf/replace` refuses a foreign main config.
- `reloadCmd` is an allow-list; `runCmd` accepts only enumerated actions.

### Changed

- SQLite persistence uses row-level upsert on the existing `records` table and reuses the connection.
- Default worker threads are 2.
- Operation history is written to `operateLog` and `/healthz` is a public liveness endpoint.
- Automatic package/acme install defaults to off.

### Documentation

- Cargo crate license matches the root Mulan PSL v1 `LICENSE`.
