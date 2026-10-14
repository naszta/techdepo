# UI design

Goal: **nice, modern, simple.** A calm, content-first interface that looks
like a well-made product, not a template. When in doubt, remove something.

## Principles

1. **Content first.** Products, specs, orders and conversations are the
   UI, and chrome stays out of the way.
2. **One accent colour.** Everything else is neutral. Colour carries
   meaning: status, compatibility and demo controls.
3. **Borders over shadows, whitespace over boxes.** Flat surfaces, thin
   borders, and generous spacing.
4. **The same thing always looks the same.** Every status, severity and
   actor has exactly one badge style, used everywhere.
5. **Demo controls never blend into the real UI.** They have their own
   unmistakable look, so it's always obvious what a customer would see
   and what is a simulation lever.
6. **Explainability is a feature.** Probabilities, traces and timelines
   are designed screens, not debug dumps.

What we won't do: gradients everywhere, glassmorphism, stock photos,
carousels, heavy shadows, more than one accent, or animations longer
than ~200 ms.

## Foundation: Tailwind CSS v4 and daisyUI 5

**Tailwind and daisyUI are the base for everything.** Styling comes from
daisyUI component classes plus Tailwind utilities. We add no custom CSS
beyond the theme definitions, the font faces, the `dark:` variant (it
follows the active daisyUI theme: an explicit `data-theme`, else the OS
setting) and one rule: the pressed button of a segmented control
(`.join > .btn.btn-active`) gets the sidebar's accent tint, because
daisyUI's `btn-active` on a neutral button is nearly invisible. That rule
is unlayered on purpose (daisyUI 5 puts `.btn` in layers that beat
`@layer components`).

| Need | daisyUI / Tailwind |
|---|---|
| Buttons | `btn`, `btn-primary`, `btn-outline`, `btn-ghost`, `btn-sm` |
| Badges, status | `badge` + `badge-soft`/`badge-outline` + semantic colour (wrapped in `StatusBadge`/`CompatBadge`) |
| Surfaces | `card card-border`, with `bg-base-100`/`bg-base-200` and `border-base-300` |
| Header | `navbar`, `indicator` (cart count), `dropdown` (Parts menu), `input` with an icon (search) |
| Account and admin layout | `drawer lg:drawer-open` + `menu` |
| Product tabs | `tabs tabs-border` |
| Order progress | `steps` |
| Timelines | `timeline timeline-vertical timeline-compact` |
| Ticket threads | `chat chat-start`/`chat-end`, `chat-bubble` |
| Admin numbers | `stats`/`stat` |
| Settings | `range` (thresholds), `join` (segmented reply policy), `toggle` (fault switches), `fieldset`/`label` (forms) |
| Feedback | `alert` + semantic colour, `toast`, `skeleton`, `loading` |
| Power bar, probabilities | `progress progress-warning` / `progress-primary` |
| Theme switch | `swap swap-rotate` with sun/moon icons, toggling `data-theme` |
| Dialogs | `modal` (through dioxus-daisyui-components' dialog) |

`apps/web/tailwind.css`:

```css
@import "tailwindcss";
@source "./src/**/*.rs";
@plugin "daisyui" { themes: false; }

@plugin "daisyui/theme" {
  name: "techdepo";
  default: true;
  prefersdark: false;
  color-scheme: light;
  --color-base-100: #ffffff;   --color-base-200: #fafafa;  --color-base-300: #e4e4e7;
  --color-base-content: #18181b;
  --color-primary: #4f46e5;    --color-primary-content: #ffffff;
  --color-neutral: #18181b;    --color-neutral-content: #fafafa;
  --color-info: #1d4ed8;  --color-success: #15803d;  --color-warning: #b45309;  --color-error: #b91c1c;
  --radius-box: 0.875rem;  --radius-field: 0.625rem;  --radius-selector: 0.5rem;
  --border: 1px;  --depth: 0;  --noise: 0;
}

@plugin "daisyui/theme" {
  name: "techdepo-dark";
  prefersdark: true;
  color-scheme: dark;
  --color-base-100: #16161a;   --color-base-200: #0c0c0f;  --color-base-300: #27272a;
  --color-base-content: #fafafa;
  --color-primary: #5850ec;    --color-primary-content: #ffffff;
  --color-neutral: #fafafa;    --color-neutral-content: #18181b;
  --color-info: #60a5fa;  --color-success: #4ade80;  --color-warning: #fbbf24;  --color-error: #f87171;
  --radius-box: 0.875rem;  --radius-field: 0.625rem;  --radius-selector: 0.5rem;
  --border: 1px;  --depth: 0;  --noise: 0;
}

@theme {
  --font-sans: "Geist", ui-sans-serif, system-ui, sans-serif;
  --font-mono: "Geist Mono", ui-monospace, monospace;
}
```

- **The demo controls** use a dedicated amber style built from Tailwind
  utilities (`border-2 border-dashed border-amber-600 bg-amber-50`, with
  dark variants). This is deliberately *not* a theme colour, so it never
  leaks into the real UI.
- **The mockup was drawn in plain HTML** to mirror these tokens. The
  implementation uses the classes above, not the mockup's inline styles.

## Theme (daisyUI 5)

- **Two custom themes,** `techdepo` (light) and `techdepo-dark`.
  - **Light/dark switch in every header** (shop, account and admin): a
    40 px icon button that shows a moon in light mode and a sun in dark
    mode, with an `aria-label` naming the target theme.
  - The first visit follows `prefers-color-scheme`. Clicking the switch
    sets `data-theme` on `<html>` and remembers the choice in
    `localStorage`.
  - A tiny inline script in `index.html` applies the saved theme before
    the WASM loads, so there is no flash of the wrong theme.
- **Neutral base:** zinc or slate greys. `base-100` is the page,
  `base-200` the subtle sections, and `base-300` the borders.
- **Accent (`primary`):** a single confident indigo, `#4f46e5` in light
  mode and `#5850ec` in dark mode (both keep white button text at AA
  contrast). It is used for primary actions, links and selection only.
  - Tints are derived with
    `color-mix(in oklab, var(--color-primary) 7%, white)`.
- **Mockup:** https://claude.ai/artifact/DH1tb9whf4S4vsauhn5bFN has the
  product page, the PC builder (dark), order detail with demo controls,
  and admin human tasks. It is private to the owner.
- **Semantic colours**, used only for meaning:

  | Colour | Meaning |
  |---|---|
  | `success` | compatible, delivered, paid |
  | `warning` | compat warning, needs attention |
  | `error` | incompatible, failed, lost |
  | `info` | notes, informational |

- **Shape:** `--radius-box: 0.75rem`, `--radius-field: 0.5rem`,
  `--radius-selector: 0.5rem`. Set `--depth: 0` and `--noise: 0` for a
  flat look. Borders are 1 px.
- **Typography:**
  - **Geist** for the UI.
  - **Geist Mono** for SKUs, order and ticket numbers, IDs and JSON.
  - Inter was dropped: it reads as a template default.
  - Tabular figures for prices and tables.
  - Fonts are self-hosted from Static Assets.
- **Type scale:** 14 px base in admin and 16 px in the shop. Headings
  are semibold, never bold-black.
- **Icons:** Lucide, one consistent stroke icon set, through
  `dioxus-free-icons` if it supports Dioxus 0.7, otherwise a small
  inline SVG module.
- **Product imagery:** no photos. Each category gets a clean line
  illustration or icon on a tinted tile, plus the fictional brand's
  wordmark in text. It is consistent, light, and has no rights issues.

## Status vocabulary (one badge per meaning, everywhere)

| Domain | Values → style |
|---|---|
| Order | `awaiting_payment` ghost · `paid`/`processing` info · `shipped` primary-soft · `delivered` success · `cancelled`/`refunded`/`returned` neutral |
| Compatibility | ✓ success · ⚠ warning · ✕ error · ℹ info. The icon and the colour always appear together, never colour alone. |
| Ticket | open · awaiting customer · with human · resolved |
| Actor | **AI** (sparkle icon) · **Human** (person icon) · **System** (gear icon). Used in ticket threads, timelines and traces. |
| Flags | `refund pending`, `investigation open` as outline badges next to the status |

## Layouts

- **Shop:** top navbar with the logo wordmark, search, a categories
  menu, **Build a PC**, the cart indicator, the Clerk
  `OrganizationSwitcher` (switch or create a shop; no personal account)
  and `UserButton`. The account and admin top bars end the same way.
  Below `md` the search moves to its own row under the navbar.
  Content is centred, with a maximum width of about 1200 px.
  Mobile-first.
- **Account and admin:** a sidebar layout (daisyUI `drawer`, collapsing
  on mobile) with a slim top bar.
  - A small switch moves between "Shop" and "Admin", since one user
    plays both roles.
  - Admin is denser (14 px, compact tables) but uses the same theme.
- **A persistent thin banner** at the very top: "Demo shop – not a real
  store", with a "How this demo works" link to the demo guide. It is
  neutral, not alarming.

## Key screens

| Screen | Design |
|---|---|
| Home | Short hero ("PC parts, explained. Builds that just work.") with two calls to action, **Build a PC** and **Browse parts**. A category grid of icon tiles. Three "why TechDepó" points. Nothing else, except that the hero badge links the demo guide and signed-out visitors get one line pointing to it. |
| Demo guide (`/demo`, the landing page) | Public. A hero ("A PC-parts shop that shows its work.") with **Sign in to try it** (or **My orders**) and **Look around the shop**. "Nothing here is real" facts. Four cards, one per answer to "who handles this?" (code, durable workflow, AI agent, a person) with examples from the shop. Six numbered steps to try it, showing the caller's support address when signed in. A "Things to try" list (a table from `md`, stacked rows on phones): preset, what to email, what happens. |
| Category | Filters in a left column (a drawer on mobile): typed facets plus a price range, and a "compatible with my build" toggle (on by default once a build exists). A product card grid shows the icon tile, name, 2–3 key specs, price, and a compatibility badge when a build exists. |
| Product | Two columns: tile and summary on the left; price, stock, **Add to cart** and **Add to build** on the right. Tabs below: Overview (the rich content), Specs (a clean two-column table), Compatibility (against the current build, with reasons) and FAQ. |
| PC builder | A slot table (category → chosen part → price → status icon, plus **Choose** or **Swap**). A sticky summary card holds the overall compatibility verdict, the expandable issues list with "fix" suggestions, a wattage bar (estimate vs PSU), the total, and **Add build to cart**. On mobile the summary card sits below the table, and a fixed bottom bar shows the verdict and the total with a link down to it. |
| Cart / checkout | A single column. Checkout is one page in three numbered sections, each titled with exactly what it asks for and a one-line hint: **Shipping address** (where the parcel goes), **Billing address** (what the invoice says; "Same as the shipping address" by default), **Payment method**. The order summary on the side lists "Shipping cost". |
| Order detail | A status stepper at the top, then cards for items, payments ("card statement"), invoices, shipment and tickets, then the event timeline. The **Demo controls** drawer opens from a floating button. |
| Support (customer) | A ticket list. The thread is shown as chat bubbles: customer on the right, TechDepó on the left, with an AI/Human/System marker. The "Send as email" composer sits at the bottom. |
| Admin overview | A stat row (orders, open tickets, pending tasks, revenue). The **Human tasks** list sits prominently below. |
| Human tasks | A split view: task list on the left, detail on the right showing the context, evidence, AI draft with confidence breakdown, decision buttons and a note. |
| Ticket (admin) | The thread, plus a side panel with the classification probabilities as small horizontal bars, the route taken, reply confidence, and **Take over** / **Hand back**. A "Trace" tab shows the agent steps as a vertical timeline: each LLM and tool call is a collapsible row with duration, tokens, and pretty-printed JSON in mono. |
| Settings | Grouped cards. Sliders (`range`) for the escalation and reply thresholds, each with the current value and a one-line explanation. The AI reply policy is a segmented control. Holidays are a date picker plus removable chips. Demo reset is in a danger-zone card at the bottom. |
| Muhaha | Only for orgs with the `muhaha` feature (the operator's): its own **Operator** section at the bottom of the admin sidebar (it acts across every org, not on the shop), hidden otherwise; the URL and its API routes are a not-found. **Models**: one row per AI task (product advisor, order assistant, triage, offer replies, judge, classifier fallback) with a provider select (providers without credentials disabled, "not configured") and a model ID field with per-provider suggestions; a hint says whether a classifier is calibrated (decision model) or JSON mode, and what the default is. Save, Restore defaults (into the form), Use the deployment's defaults (forgets the saved settings). **Shops**: every org with members, orders, tickets and whether its stock follows the current catalog; per row Reseed stock (confirm dialog) and Clear data (type `RESET`), and Reset every shop (type `RESET`) in the panel header. |

## Demo controls style

- A right-side drawer with an **amber dashed border**, a flask icon, and
  a "Demo controls" title with a short subtitle: "Simulate what happens
  outside the shop".
- **It never hides the page.** From `lg` on the open drawer sits beside the
  content (the layout reserves its width), and pages under it lay out by
  their content width (container queries), so a laptop with the drawer
  open gets the narrow layout instead of cards under the drawer. Below
  `lg` it is a modal with a backdrop.
- **Contents, in order:**
  1. The simulated clock, with its quick buttons.
  2. Valid next actions, as buttons.
  3. Fault toggles for the next step, as `toggle` inputs with one-line
     descriptions.
  4. "Hidden truth", e.g. "Parcel is lost: outcome *gone*".
- **Actions that invalidate a state are hidden, not disabled.** Only
  what's possible is shown.
- Scenario presets (M7) live here too, and in admin.

## Interaction details

- **Toasts** for completed actions. **Skeletons** while loading, never
  spinners on a whole page.
- **Optimistic UI only for local things** (cart, builder). Anything that
  goes through Restate shows a pending state until the API confirms.
- **Empty states** have one sentence and one call to action, e.g. "No
  orders yet. **Build a PC**".
- **Copy:** short, friendly and precise. Technical terms are allowed,
  since the audience builds PCs. Explain *why* in compatibility
  messages.

## Accessibility and responsiveness

- WCAG AA contrast in both themes, visible focus rings, and keyboard
  navigation (`dioxus-primitives` provides most of this).
- Colour is never the only signal: icon plus text always.
- `prefers-reduced-motion` is respected.
- The shop and account pages are fully responsive. Admin is
  desktop-first but usable on a tablet, and tables scroll horizontally
  on small screens.

## Implementation notes

- **Components come from
  [dioxus-daisyui-components](https://github.com/sagikazarmark/dioxus-daisyui-components).**
  It is a good fit:
  - It ships no CSS and no theme. It only emits daisyUI classes, so our
    `techdepo` themes restyle everything.
  - It is tested on browser CSR, which is exactly our target (a SPA).
  - Its `Field`/`Fieldset` sit on `dioxus-field`, the same family as
    dioform, so forms connect cleanly.
  - Behaviour (focus, keyboard, ARIA) comes from `dioxus-primitives`.
- **Components to install,** about 25 of its 60:
  - Layout and navigation: `navbar`, `drawer`, `breadcrumbs`, `tabs`,
    `dropdown_menu`, `indicator`, `avatar`
  - Display: `card`, `badge`, `status`, `table`, `stat`, `steps`,
    `timeline`, `chat`, `progress`, `skeleton`, `loading`, `alert`,
    `toast`, `tooltip`
  - Forms and overlays: `button`, `input`, `textarea`, `native_select`,
    `radio_group`, `switch`, `slider`, `toggle_group`, `field`,
    `fieldset`, `label`, `dialog`
- **How to install:**
  - Run `dx components add <name> --path <checkout>` from a checkout
    pinned to a known commit. The registry installs from a local path
    until its git registry ships.
  - Components are **copied** into `apps/web/src/components/` and owned
    by us from then on. Re-running the command updates them.
  - Installing adds a git dependency on `DioxusLabs/components`
    (`dioxus-primitives`), which has to be on the `cargo deny` git
    allow-list.
- **One exception, the theme switch.** The registry's `theme_controller`
  is CSS-only (`:has(:checked)`): it can't remember the choice or prevent
  a flash of the wrong theme before the WASM loads. We use a small
  `ThemeToggle` instead: a daisyUI `swap` button that sets `data-theme`
  on `<html>` and saves the choice to `localStorage`, plus the inline
  pre-load script.
- **App-level components on top:**
  - `StatusBadge`, `CompatBadge`, `ActorTag`, `Money`, `Mono`,
    `ThemeToggle`
  - `TraceStep`, `DemoDrawer`, `ProbabilityBars`
- **No inline colour classes for meaning.** Everything goes through
  these components, so the status vocabulary stays consistent.
- **Forms:** dioform with the registry's `Field`, `FieldLabel`,
  `FieldError` and controls, and inline validation messages under the
  fields.
- **In M0, build the theme and app shell first:** navbar, sidebar layout,
  both themes and the demo banner. Every later milestone then lands in a
  finished-looking frame.
