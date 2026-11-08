# nox website

Static landing page, served and bundled with [Bun](https://bun.sh).

```sh
bun run dev       # dev server with hot reload (http://localhost:3000)
bun run build     # production build into dist/
bun run preview   # build, then serve dist/ (PORT env var, default 3000)
```

The version pill in the download section (`v0.1.2`) is plain HTML: bump it with each release.
`llms.txt` (an [llmstxt.org](https://llmstxt.org) index), `llms-full.txt` (features, agent CLI,
shortcuts and config in plain text) and `robots.txt` are copied into `dist/` as is. Keep the
`llms` files in sync with `index.html` and SPEC.md.
