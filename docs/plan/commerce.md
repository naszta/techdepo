# Commerce: catalog, compatibility, shop, orders, simulation

This document covers everything except customer support: the catalog,
the compatibility engine, the storefront and PC builder, the order
lifecycle with its demo controls and fault injection, and both dashboards.

## 1. Tenancy model

- A **Clerk organization is one webshop instance.** Every row in D1 is
  scoped by `org_id`. The product catalog is the one exception: it is
  global and read-only.
- **One user is both the customer and the admin.** The org creator is
  `org:admin` and also shops in the same org, switching between
  `/account` and `/admin`.
  - Admin routes and APIs still check `org:admin`.
  - Orders, carts and tickets are keyed by `user_id`.
  - Invitations (separate `org:member` customers) can be added later
    without schema changes.
- **Locale:** English UI and EUR prices.
  - Prices are VAT-inclusive, with a single VAT rate per org. The default
    is configurable; I'd use 27 % for the Hungarian flavour.
  - Invoices show the net and VAT breakdown.
  - `Money` is integer cents.
- **Public org ID.** Clerk org IDs (`org_2Zx…`) are long and mixed-case,
  which is risky in an email local part. Many systems lower-case or mangle
  them. Each org therefore gets a short, lower-case public ID
  (`[a-z0-9]{8}`) stored in D1, so the support address is
  `support+k3x9q2ab@techdepo.dev`.
- **Lazy provisioning.** The first authenticated API call for an unknown
  org upserts the `orgs` row, seeds per-org inventory from catalog
  defaults, and writes default settings. MVP needs no Clerk webhooks.
  Later, `organization.deleted` can be handled for cleanup.
- **Demo reset.** An admin button wipes all of the org's orders, tickets
  and invoices and reseeds stock. Demos get messy, so this is essential.

## 2. Product catalog

### Decision: curated and fictional, about 10 per category

See the research notes in [README](README.md#research-notes).

- **Use fictional brands on real standards.** Brands are made up
  ("Voltaris" CPUs, "Kestrel" boards, "Nimbus" memory, …). Sockets, DDR
  generations, form factors and connectors are real: AM5, AM4, LGA1851,
  DDR4/DDR5, ATX/mATX/ITX, 12V-2x6, SFX.
  - This avoids trademark and endorsement problems, scraped-data licensing
    and image rights, and arguments about whether a spec is right.
  - It also lets us design the compatibility traps on purpose.
- **Why not an existing dataset:**
  - The free datasets are either scraped from PCPartPicker (unclear
    provenance) or CC BY-NC-SA (BuildCores OpenDB, the most complete one).
  - None of them has every attribute a correct checker needs, such as case
    clearances, cooler socket lists, PSU connectors and BIOS support.
  - BuildCores' JSON Schemas are still a good reference for which
    attributes to model.
- **Size:** 8 categories × 10–12 products is about 90 products.
  - More products don't improve the demo.
  - What matters is that every interesting situation exists, so "suggest
    an alternative in the same price range" always has an answer: at
    least two options per price tier in each category, at least two per
    platform in each platform-bound category, and for every platform-bound
    product an alternative on its platform in the same or an adjacent
    tier. Two or three per tier on *every* platform would double the
    CPUs and boards for little demo value. The demo-scenario kits get an
    explicit same-price alternative (the DDR4 kits have DDR5 kits within
    about 20 % of their price).
  - It also needs at least one deliberate trap per compatibility rule.
  - 90 products also lets us test every pair exhaustively.

### Categories and the attributes the rules engine needs

| Category | Compatibility-relevant attributes |
|---|---|
| CPU | socket, family/generation, TDP + peak power, supported memory types with the official speed for two and for four modules, iGPU (bool), bundled cooler (bool) |
| Motherboard | socket, chipset, form factor, memory type, slots, max memory, M.2 slots (key, length, gen), SATA ports, min BIOS per CPU family, BIOS flashback (bool), CPU power connectors |
| Memory | type, modules × capacity, speed, height |
| Storage | form factor (M.2 2280 / 2.5" / 3.5"), interface (NVMe gen / SATA) |
| GPU | length, slot width, board power, power connectors, PCIe gen |
| Case | supported MB form factors, max GPU length + thickness (slots), max cooler height, PSU form factor + max length, radiator mounts (size per position), drive bays |
| PSU | wattage, form factor, length, connectors (EPS, PCIe 8-pin, 12V-2x6, SATA) |
| CPU cooler | supported sockets, height and memory clearance under the fan (air) or radiator size (AIO), TDP rating |

### Representation

- Product data lives in the repo as one YAML file per category under
  `data/catalog/*.yaml`.
- The `catalog` crate deserializes it into **typed** specs. The value
  types are enums, never free strings (`Socket::Am5`, `MemoryType::Ddr5`,
  `FormFactor::MicroAtx`):

  ```rust
  enum ProductSpec { Cpu(CpuSpec), Motherboard(MotherboardSpec), /* … */ }
  ```

- An `xtask seed` command validates the catalog and generates the D1
  seed SQL.
- **Rich content per product:** a long-form description, "who is it for",
  pros/cons, and an FAQ.
  - It is generated together with the specs during implementation and
    committed as Markdown next to the YAML.
  - There is no manual review. `xtask catalog validate` enforces
    plausibility, coverage and completeness (see
    [roadmap.md M1](roadmap.md#m1-catalog-and-compatibility)).
  - This content is what makes the shop "excel at useful information",
    and it is what the agents read through `get_product`.
- **Images:** generated or abstract illustrations per category. No
  product photos.

## 3. Compatibility engine (`compat` crate)

"Perfect compatibility" here means a **closed world**: we only model
attributes we check, and we test every combination in the catalog.

- **Pure Rust with no I/O.**
  - It is compiled into the SPA, so the builder gives instant feedback.
  - It runs in the API worker to validate the cart at checkout.
  - It is exposed as an agent tool, so the agent never "guesses"
    compatibility.
- **API:**

  ```rust
  fn check_build(build: &Build) -> Report   // all rules
  fn check_pair(a: &Product, b: &Product) -> Report  // build with just these two
  fn alternatives(build: &Build, slot: Category, price_band: Range<Money>) -> Vec<ProductId>

  struct Issue { severity: Error | Warning | Note, rule: RuleId, parts: Vec<ProductId>, message: String }
  ```

- **Severity** (UX modelled on PCPartPicker):
  - `Error` means it physically or electrically won't work. It blocks
    checkout unless the user explicitly overrides it.
  - `Warning` means it works with caveats, such as a BIOS update or PSU
    headroom under 20 %.
  - `Note` is informational, e.g. "RAM will run at 5600 MT/s".
- **Rules for v1:**
  - **Errors:**
    - CPU↔MB socket
    - MB↔RAM type
    - RAM module count above slots, or capacity above MB max
    - case↔MB form factor
    - GPU length and thickness (slots) vs case
    - air cooler height vs case, including a fan raised over tall memory
    - cooler socket support
    - AIO radiator vs case mounts
    - PSU form factor vs case (an SFX unit in an ATX-only case is a
      warning instead: it needs an SFX-to-ATX bracket)
    - PSU wattage below estimated load
    - GPU power connectors not available on the PSU
    - M.2 drives above M.2 slots
    - SATA drives above SATA ports
  - **Warnings:**
    - BIOS update needed (worse when the board has no flashback)
    - PSU headroom under 20 %
    - cooler rating below the CPU's sustained peak power (not its TDP:
      some CPUs sold as 125 W parts draw about 250 W)
    - memory taller than an air cooler's clearance (the fan has to be
      raised)
    - SFX power supply in an ATX-only case (needs a bracket)
    - fewer EPS cables than 8-pin CPU headers, with a CPU drawing more
      than 200 W
    - CPU without iGPU and no GPU in the build
    - no cooler, and the CPU has no bundled cooler
  - **Notes:** RAM speed above what the platform supports, including the
    lower official speed with four modules (two per channel), and
    similar downclocking cases (mixed kits, M.2 and GPU link speed,
    PSU below the GPU vendor's recommendation).
- **Wattage estimate:** sum of components plus 10 %, shown in the
  builder.
- **Tests:**
  - Unit tests per rule.
  - An **exhaustive pairwise matrix** over the whole catalog
    (about 90² pairs, which is cheap). It is stored as a snapshot
    (`insta`) and reviewed by a person whenever the catalog changes.
  - A handful of golden full builds, including the "trap" builds used in
    demos.

## 4. Storefront (customer-facing)

Everything that belongs to a shop (stock, cart, builds saved to the
account, checkout, orders, support, admin) sits behind Clerk sign-in.
Two things are public: browsing the catalog (it is compiled into the
SPA, so there is nothing to protect) and the **demo guide** at `/demo`,
the landing page that explains the demo: what is not real, which steps
are code, a durable workflow, an AI agent or a person, and how to drive
it (sign in, demo controls, scenario presets, the support address,
admin). The permanent demo banner links to it on every page. Pages:

- **Catalog:** browse by category, with faceted filters derived from the
  typed specs (socket, memory type, form factor, price). It has a "show
  only parts compatible with my build" toggle, on by default once a build
  exists.
- **Product page:**
  - Specs table, rich content and FAQ.
  - Badge: "compatible with your current build" ✓, ⚠ or ✗, with reasons.
  - "Works well with" list.
  - Stock level, which is per org.
- **Compare:** two or three products side by side. Cheap to build and
  great for "useful information".
- **PC builder:**
  - Slot-based, like PCPartPicker: one slot per category, and multiple
    for storage and RAM.
  - Live report from `compat::check_build`, wattage estimate and total
    price.
  - "Fix it" suggestions from `compat::suggest_fixes`: every candidate is
    re-checked against the whole build, so a suggestion never brings a new
    error, and never a new warning unless it is the only way to clear an
    error (the suggested PSU for a wattage error must not then warn about
    headroom when a bigger one fits).
  - Save named builds, and add a whole build to the cart.
- **Cart:**
  - Persisted server-side per user.
  - Shows a compatibility report when the cart contains build-relevant
    parts.
  - Stock check.
- **Checkout** (dioform):
  - Shipping address (EU countries only, `domain::SHIPPING_COUNTRIES`;
    shipping is free), plus a billing address with a "same as shipping"
    option. A placed order's addresses never change: before it ships,
    the customer cancels and orders again.
  - Simulated payment method.
  - "Place order" creates the order in `awaiting_payment`.

## 5. Order lifecycle

### The `Order` Restate virtual object owns the state machine

The object is keyed by order ID. All mutations go through it, so they are
serialized per order. It writes the D1 read model and appends to
`order_events` inside `ctx.run`. The UI reads D1.

```
                 ┌──────────── cancel (unpaid) ───────────► cancelled
                 │
awaiting_payment ─[pay]─► paid ─(auto)─► processing ─[ship]─► shipped ─[time passes]─► delivered
                            │               │                    │                         │
                            │      (auto) confirmation email     │ (hidden) lost           └─► return_requested ─► returned
                            │      (auto) invoice issued         │
                            └──── refund (human-approved) ───────┴──► refunded
```

- **Customer-visible statuses:** `awaiting_payment`, `paid`,
  `processing`, `shipped`, `delivered`, `cancelled`, `refunded`,
  `returned`.
- **Overlay flags** sit next to the status (`refund_pending`,
  `investigation_open`, `replacement_for: <order>`).
- **The shipment carries hidden truth** in the `lost` flag. The customer
  only ever sees "in transit".
- **Cancel instead of delete.** A cancelled order is soft-deleted: it is
  hidden from default lists but kept for invoices and audit. This
  replaces the "delete my order" idea from the brief.

### Steps, automation, and injectable faults

Each manual step is a button in the **demo controls** panel. Automatic
steps happen by themselves, and faults can be toggled before triggering
the step that would hit them.

| Step | Trigger | Faults | Fix path |
|---|---|---|---|
| Pay | manual: "Simulate payment" | **Charge twice**: two captured charges. **Payment declined**: the order stays unpaid and the customer can retry. | Customer emails about the double charge, the support workflow verifies it, a human approves the refund, and the duplicate is refunded. |
| Confirmation email | auto, after payment | **Email provider outage for N minutes**: the transport fails and Restate retries until it recovers. This shows durability in the Restate UI. | none needed |
| Issue invoice | auto, after payment (EU prepaid practice) | **Missing billing address** on the invoice | Customer complains, a workflow voids it and issues a corrected one (automatic, no money moves). An admin can also do this manually. |
| Ship | manual: "Mark as shipped" | **Lost in transit**, with outcome: `resurfaces` (the carrier finds it, delivering the next workday) or `gone` (no information) | WISMO policy, then investigation, then a human decision (see [support.md](support.md)) |
| Deliver | automatic when simulated time passes the carrier's ETA, unless lost | (later) "delivered but not received", "damaged" | (later) |
| Return | manual: "Customer sends parcel back" (allowed after a return request) | none | Order moves to `returned`, and `RefundWorkflow` starts. Approval is still required. |

Faults that can come later: out of stock after payment (oversell, leading
to backorder or partial refund), wrong item shipped, wrong VAT on the
invoice, and a carrier API that is down (shows retries).

### Simulated time

- **Each order has a virtual clock**, an offset from real time. With a
  per-order clock, one order can sit at day 2 while another sits at day 6
  in the same demo.
- **Controls:** `+1 day`, `+1 workday`, `→ next Monday 09:00`, and "set
  ship date to last Friday". The panel shows "Simulated now: Mon
  2026-10-19 09:00".
- **Delivery:** the carrier ETA is ship date + 1–3 workdays, chosen
  deterministically per shipment. When the clock advances past the ETA,
  the `Order` object asks the carrier, and if the parcel isn't lost it
  delivers.
- **Workdays** are Monday–Friday. A holiday list is configurable and
  empty by default: `OrgSettings::holidays` (`YYYY-MM-DD` dates, edited on
  the admin settings page), turned into the org's `WorkCalendar` by
  `OrgSettings::calendar()`. Every workday computation uses it: the carrier
  ETA (fixed when shipped or when the ship date moves), a trace's "next
  workday", `+1 workday`, and the workdays since shipment in the shipping
  policy, the investigation evidence and the demo panel. Changing the list
  does not move ETAs already given.
- **Real Restate timers** are kept for real-time concerns: reminder
  emails, human-task SLAs, and closing tickets after customer silence.
  Each has a short, demo-configurable duration.
- **Shipping policy** is a pure function in `domain`, used by the
  support workflow. `n` is the number of full workdays since shipment:

  | Workdays since shipment `n` | Response |
  |---|---|
  | ≤ 3 | Within the standard 1–3 workday estimate. Reply with the policy and the ETA. |
  | 4 | Carrier is overloaded. Ask the customer to wait a little longer. |
  | ≥ 5 | Acknowledge the problem and open an investigation. |

  Shipped on Friday with a complaint on Monday gives `n = 1`, so the
  customer gets the standard policy.

### Payments: a fake payment service provider

- **Model:** a `payments` ledger of charges and refunds per order, with a
  provider reference, amount and status.
- **The customer's order page shows a "card statement" view.** That is
  how they notice a double charge.
- **Refunds** are only executed by the `RefundWorkflow`, which is always
  human-approved, or pre-approved when a human made the offer.

### Invoices

- **Invoices are immutable documents.** A fix is a **void** (a storno or
  credit note that references the original) plus a new invoice that
  references what it replaces. This matches real accounting practice and
  makes a good demo artifact.
- **Numbering:**
  - Sequential per org, e.g. `TD-2026-000042`.
  - Allocated with an atomic D1 `UPDATE … RETURNING` inside `ctx.run`,
    so replays don't skip numbers.
- **Refunds of invoiced orders** produce a credit note, except the refund
  of a duplicate charge: the invoice covers the order once, and the
  duplicate was never invoiced (`RefundReason::credits_invoice`).
- **Rendered as HTML** in both dashboards and attached to the
  confirmation email as HTML. PDF can come later.

### Carrier (simulated "delivery company")

- A small fake carrier with its own table: tracking number, ETA, status,
  scan events, and the hidden lost outcome.
- It exposes `track(tracking_no)` and `open_trace(tracking_no)`.
  `open_trace` reveals `resurfaces` (ETA becomes the next workday) or `gone`
  (no information). A trace takes the carrier a short while (15 s of
  real time): until then `open_trace` answers `open`, and the
  `Investigation` workflow polls it.
- **It has an injectable "API unavailable" fault** to show retries and
  journaling.
- The support workflow calls the same functions (see
  [support.md](support.md)).

## 6. Customer dashboard (`/account`)

- **Orders:** a list, plus a detail page with:
  - the status timeline from `order_events`
  - items, addresses, payments (the card statement view), invoices,
    shipment tracking, and linked tickets
  - **"Contact support about this order"**: a `mailto:` link to the
    org's support address, with the subject pre-filled with the order
    number.
- **Demo controls drawer**, on the order detail page:
  - Visually distinct from the real UI (amber, striped, labelled "Demo
    controls").
  - Shows only the *valid next actions* for the current state, fault
    toggles for the next step, and the simulated clock.
  - Shows hidden truth such as "parcel is lost (outcome: gone)".
- **Support:**
  - The customer's tickets with status and the conversation, read-only.
  - A **"Send as email" web form** that injects a message into the same
    pipeline as a real email. Useful when presenting without an email
    client, and for local development.
- **Builds:** saved PC builds.
- **Profile:** addresses. Identity is managed by Clerk.

## 7. Admin dashboard (`/admin`, `org:admin` only)

- **Overview:**
  - KPIs: orders by status, open tickets, pending human tasks, simulated
    revenue, refunds.
  - Recent activity feed.
- **Human tasks inbox.** This is the most important admin page. It is one
  queue holding:
  - refund approvals
  - investigation decisions
  - escalations
  - AI reply drafts awaiting review
  - "customer rejected every offer"

  Each task shows its context and evidence, offers the allowed decisions,
  and has a free-text note.
- **Orders:**
  - Filterable list.
  - Detail page: items, payments ledger, invoices (void/reissue), shipment
    and carrier events, event timeline, linked tickets, investigations and
    refunds.
  - Admin actions: cancel, initiate refund, and so on.
- **Tickets:**
  - Conversation thread.
  - Classification with per-intent probabilities.
  - Assigned route (workflow X or agent Y).
  - **Agent trace**: every LLM call and tool call with inputs and
    outputs. This is what makes the demo explainable.
  - Reply manually, take over from the AI, or hand back to the AI.
- **Investigations:**
  - Their own list, because they can exist without a ticket when an
    admin opens one from an order.
  - Each one links to its ticket and order.
  - Shows state and carrier responses.
- **Refunds:** requested, approved or rejected, and completed, with
  links.
- **Invoices:** all invoices and credit notes, filterable by
  issued/voided.
- **Payments:** the ledger, with the duplicate-charge detector
  highlighting suspicious pairs. It is deterministic, with no AI.
- **Products & inventory:**
  - The read-only global catalog.
  - **Editable per-org stock**, with low-stock highlighting.
  - Recommendation: **no price editing**. Products are global, and
    per-org price overrides add complexity without demo value.
  - Editable stock is cheap and enables oversell and backorder
    scenarios later.
- **Customers:** org members from Clerk, with their orders and tickets.
- **Email log:**
  - Every outbound email, as a copy of what was sent.
  - Every inbound email, **including rejected ones** (unknown sender,
    failed DMARC, auto-replies), with the reason.
- **Settings:**
  - Escalation confidence threshold (slider, default 0.70).
  - AI reply policy: `always_send`, `always_review`, or
    `send_above_confidence` with its own threshold slider (see
    [support.md §6](support.md#ai-reply-policy-org-setting)).
  - Default fault toggles.
  - Real-time durations for reminders and timeouts.
  - Holidays (the workday calendar, see §5 "Simulated time").
  - Demo reset.
