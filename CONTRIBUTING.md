# Contributing

This project is a small Rust + SvelteKit rewrite of nginxWebUI. Please keep changes focused.

## Development

- Backend: `cd server && cargo test`
- Frontend: `cd web && pnpm install && pnpm run dev` (proxies `/adminPage` to port 8080)
- Frontend checks: `cd web && pnpm run check && pnpm run build`

Use the existing code style. Prefer adding or updating tests next to the code you change (`#[cfg(test)]` in the same module).

## Scope

Daily site management writes Debian-style files under `sites-available`. The advanced HTTP / server editors only apply when this app generated the main `nginx.conf`. Do not delete the old server CRUD unless that product decision is explicit.

Do not add remote cluster, SSO, mail alerts, extra cloud DNS plugins, or Traditional Chinese UI — those are intentionally out of scope (`DIFF.md`).

## License

The repository is licensed under the Mulan Permissive Software License, Version 1 (`LICENSE`, SPDX `MulanPSL-1.0`). Pull requests are accepted under the same license.

## Security

Please report security issues privately to the maintainers instead of opening a public issue with an exploit.
