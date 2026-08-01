# agent-ctrl website

The public Next.js site for agent-ctrl.

## Develop

To add components to your app, run the following command:

```bash
npm ci
npm run dev
```

## Verify

```bash
npm run typecheck
npm run lint
npm run build
```

GitHub Pages sets `AGENT_CTRL_STATIC_EXPORT=1` and
`NEXT_PUBLIC_BASE_PATH=/agent-ctrl`; the deployment workflow uploads
`site/out`, not the application source.
