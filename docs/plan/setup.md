# Account setup

Moved out of the plan, now that the accounts are being configured:

- [docs/deploy.md](../deploy.md): the two deployments (`dev` on
  `techdepo-dev-web.vibekoders.workers.dev`, `live` on `techdepo.dev`), the
  one-time Cloudflare, Restate Cloud, Clerk, Email Routing, Resend and GitHub
  setup per environment, the workers and their secrets, model configuration,
  and how a deploy registers the services worker's preview URL with Restate.
- [docs/local-development.md](../local-development.md): running the whole
  stack locally without accounts, and opting into Workers AI, Clerk and the
  eval providers.
