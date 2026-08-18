# vendor/

Third-party frontend assets checked into the repo so the webview never
fetches anything at runtime (the app's CSP is `default-src 'self'` on
purpose — no CDN, no network dependency for the UI to render correctly
offline or on a LAN with no internet route).

- `bootstrap-icons/` — [Bootstrap Icons](https://icons.getbootstrap.com/)
  v1.13.1, MIT licensed (see `bootstrap-icons/LICENSE.md`). Only
  `bootstrap-icons.min.css` and `fonts/bootstrap-icons.woff2` are vendored —
  the per-icon SVG files and the `.woff` fallback (superseded by `.woff2`
  everywhere this app runs) aren't needed and were left out.

To upgrade: download a newer release zip from
https://github.com/twbs/icons/releases, and replace both files with the
same two paths from the new archive. Check the license file for changes too.
