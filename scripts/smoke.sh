#!/usr/bin/env bash
# End-to-end smoke test of techdepo-web (/api/*) against the local stack.
#
# Needs: Restate + Mailpit (`docker compose -f deploy/compose.yaml up -d`), the services worker
# on :8788 registered with Restate, and the web worker on :8787 with AUTH_MODE=dev, both with
# the same `--persist-to ../../.wrangler/state` (see CLAUDE.md). Uses jq if available, else
# python3.
#
#   scripts/smoke.sh                     # API=http://localhost:8787
#
# Every run signs in to a fresh dev shop ("Smoke <timestamp>"), so runs do not interfere.

set -euo pipefail

API=${API:-http://localhost:8787}
RUN=$(date +%s)
TOKEN=""
STATUS=""
BODY=""

# -- helpers -------------------------------------------------------------------------------------

# jx FILTER: read JSON on stdin, print the value at FILTER (`.a.b[0].c`, optionally `| length`).
if command -v jq >/dev/null 2>&1; then
  jx() { jq -r "$1"; }
else
  jx() {
    python3 -c '
import json, re, sys
data = json.load(sys.stdin)
expr = sys.argv[1].strip()
length = expr.endswith("| length")
if length:
    expr = expr[: -len("| length")].strip()
v = data
for key, idx in re.findall(r"\.([A-Za-z_][A-Za-z0-9_]*)|\[(\d+)\]", expr):
    if v is None:
        break
    v = v.get(key) if key else (v[int(idx)] if int(idx) < len(v) else None)
if length:
    v = 0 if v is None else len(v)
print("null" if v is None else (json.dumps(v) if isinstance(v, (dict, list)) else (str(v).lower() if isinstance(v, bool) else v)))
' "$1"
  }
fi

uuid() { printf 'smoke-%s-%s' "$RUN" "$((RANDOM * 32768 + RANDOM))"; }

step() { printf '\n\033[1m== %s\033[0m\n' "$*"; }
ok() { printf '   ok: %s\n' "$*"; }
fail() { printf '\033[31mFAIL: %s\033[0m\n' "$*" >&2; [ -n "$BODY" ] && printf '%s\n' "$BODY" >&2; exit 1; }

# call METHOD PATH [JSON]: sets STATUS and BODY. Authenticated when TOKEN is set.
call() {
  local method=$1 path=$2 data=${3:-}
  local args=(-s -o /tmp/smoke.$$ -w '%{http_code}' -X "$method" "$API$path" -H 'accept: application/json')
  [ -n "$TOKEN" ] && args+=(-H "authorization: Bearer $TOKEN")
  [ -n "$data" ] && args+=(-H 'content-type: application/json' -d "$data")
  STATUS=$(curl "${args[@]}") || fail "$method $path: cannot connect to $API"
  BODY=$(cat /tmp/smoke.$$)
  rm -f /tmp/smoke.$$
}

# api METHOD PATH [JSON]: like call, but fails unless 2xx; prints the body.
api() {
  call "$@"
  case $STATUS in 2*) ;; *) fail "$1 $2 answered $STATUS" ;; esac
  printf '%s' "$BODY"
}

# expect STATUS METHOD PATH [JSON]: fails unless the status matches.
expect() {
  local want=$1
  shift
  call "$@"
  [ "$STATUS" = "$want" ] || fail "$1 $2: expected $want, got $STATUS"
  ok "$1 $2 -> $STATUS $(printf '%s' "$BODY" | jx '.code' 2>/dev/null || true)"
}

eq() { [ "$1" = "$2" ] || fail "$3: expected '$2', got '$1'"; ok "$3 = $1"; }

# poll SECONDS METHOD PATH FILTER WANT: polls until FILTER equals WANT.
poll() {
  local secs=$1 method=$2 path=$3 filter=$4 want=$5 got=""
  for _ in $(seq "$secs"); do
    call "$method" "$path"
    if [[ $STATUS == 2* ]]; then
      got=$(printf '%s' "$BODY" | jx "$filter")
      [ "$got" = "$want" ] && { ok "$path $filter = $want"; return 0; }
    fi
    sleep 1
  done
  fail "$path: $filter is '$got' after ${secs}s, expected '$want'"
}

ADDRESS='{"name":"Smoke Tester","line1":"Andrássy út 1","postal_code":"1061","city":"Budapest","country":"HU"}'

# -- session -------------------------------------------------------------------------------------

step "config and dev login"
CONFIG=$(api GET /api/config)
eq "$(printf '%s' "$CONFIG" | jx .auth_mode)" dev "auth_mode"
expect 401 GET /api/me
LOGIN=$(api POST /api/dev/login "{\"email\":\"smoke+$RUN@example.com\",\"name\":\"Smoke Admin\",\"org_name\":\"Smoke $RUN\"}")
TOKEN=$(printf '%s' "$LOGIN" | jx .token)
ORG=$(printf '%s' "$LOGIN" | jx .me.org.id)
ok "org $ORG, support $(printf '%s' "$LOGIN" | jx .me.org.support_address)"
ME=$(api GET /api/me)
eq "$(printf '%s' "$ME" | jx .role)" org:admin "role"

# -- catalog -------------------------------------------------------------------------------------

step "catalog, stock, product, compatibility"
CATALOG=$(api GET /api/catalog)
PRODUCT=$(printf '%s' "$CATALOG" | jx '.products[0].id')
ok "$(printf '%s' "$CATALOG" | jx '.products | length') products, using $PRODUCT"
api GET /api/stock >/dev/null; ok "stock"
api GET "/api/products/$PRODUCT" >/dev/null; ok "product page"
api POST /api/compat/check "{\"product_ids\":[\"$PRODUCT\"],\"include_fixes\":true}" >/dev/null; ok "compat check"
expect 404 GET /api/products/no-such-product

# -- cart, builds, addresses, checkout -----------------------------------------------------------

step "cart, builds, addresses"
api POST /api/cart/items "{\"product_id\":\"$PRODUCT\",\"quantity\":2}" >/dev/null
CART=$(api PUT "/api/cart/items/$PRODUCT" '{"quantity":1}')
TOTAL=$(printf '%s' "$CART" | jx .total)
eq "$(printf '%s' "$CART" | jx '.lines[0].quantity')" 1 "cart quantity"
expect 422 POST /api/cart/items "{\"product_id\":\"$PRODUCT\",\"quantity\":0}"
BUILD=$(api POST /api/builds "{\"idempotency_key\":\"$(uuid)\",\"name\":\"Smoke build\",\"product_ids\":[\"$PRODUCT\"]}")
BUILD_ID=$(printf '%s' "$BUILD" | jx .id)
eq "$(api GET /api/builds | jx '.builds | length')" 1 "saved builds"
api PUT "/api/builds/$BUILD_ID" '{"name":"Renamed build"}' >/dev/null; ok "build renamed"
api DELETE "/api/builds/$BUILD_ID" >/dev/null; ok "build deleted"
api POST /api/addresses "{\"idempotency_key\":\"$(uuid)\",\"label\":\"Home\",\"address\":$ADDRESS,\"default_shipping\":true}" >/dev/null
eq "$(api GET /api/addresses | jx '.addresses | length')" 1 "addresses"

step "checkout (and the same idempotency key again)"
KEY=$(uuid)
CHECKOUT="{\"idempotency_key\":\"$KEY\",\"shipping_address\":$ADDRESS,\"payment_method\":\"card\",\"expected_total\":$TOTAL}"
expect 409 POST /api/checkout "{\"idempotency_key\":\"$(uuid)\",\"shipping_address\":$ADDRESS,\"payment_method\":\"card\",\"expected_total\":1}"
ORDER=$(api POST /api/checkout "$CHECKOUT" | jx .order.number)
ok "placed $ORDER"
eq "$(api POST /api/checkout "$CHECKOUT" | jx .order.number)" "$ORDER" "same key, same order"
eq "$(api GET /api/cart | jx '.lines | length')" 0 "cart emptied"
eq "$(api GET /api/orders | jx '.items[0].number')" "$ORDER" "order list"

# -- demo controls -------------------------------------------------------------------------------

step "pay with 'charge twice', ship, deliver"
PANEL=$(api PUT "/api/orders/$ORDER/demo/faults" '{"faults":{"payment":{"kind":"charge_twice"}}}')
eq "$(printf '%s' "$PANEL" | jx '.actions[0].armed_fault')" "Charge twice" "armed fault"
api POST "/api/orders/$ORDER/demo/actions" '{"action":"pay"}' >/dev/null
DETAIL=$(api GET "/api/orders/$ORDER")
eq "$(printf '%s' "$DETAIL" | jx '.payments | length')" 2 "two charges on the card statement"
eq "$(printf '%s' "$DETAIL" | jx '.invoices | length')" 1 "invoices"
INVOICE=$(printf '%s' "$DETAIL" | jx '.invoices[0].number')
INVOICE_ID=$(printf '%s' "$DETAIL" | jx '.invoices[0].id')
api GET "/api/orders/$ORDER/invoices/$INVOICE" >/dev/null; ok "invoice document $INVOICE"
expect 409 POST "/api/orders/$ORDER/demo/actions" '{"action":"pay"}'
ACTIONS=$(api GET "/api/orders/$ORDER/demo" | jx '.actions')
if printf '%s' "$ACTIONS" | grep -q start_processing; then
  api POST "/api/orders/$ORDER/demo/actions" '{"action":"start_processing"}' >/dev/null; ok "processing"
fi
api POST "/api/orders/$ORDER/demo/actions" '{"action":"ship"}' >/dev/null
eq "$(api GET "/api/orders/$ORDER" | jx .order.status)" shipped "status"
for _ in 1 2 3 4 5 6 7; do
  STATE=$(api POST "/api/orders/$ORDER/demo/clock" '{"step":"workday"}' | jx .order.status)
  [ "$STATE" = delivered ] && break
done
eq "$STATE" delivered "status after clock steps"
eq "$(api GET "/api/orders/$ORDER" | jx '.shipment.status')" delivered "tracking"

# -- admin ---------------------------------------------------------------------------------------

step "admin: overview, ledger, invoices, void and reissue"
api GET /api/admin/overview >/dev/null; ok "overview"
LEDGER=$(api GET /api/admin/payments)
eq "$(printf '%s' "$LEDGER" | jx '.duplicates[0].order_number')" "$ORDER" "duplicate flagged"
DUPLICATE=$(printf '%s' "$LEDGER" | jx '.duplicates[0].duplicate')
api GET /api/admin/invoices >/dev/null; ok "invoice list"
api GET "/api/admin/invoices/$INVOICE_ID" >/dev/null; ok "invoice document"
REISSUED=$(api POST "/api/admin/invoices/$INVOICE_ID/void-reissue" "{\"reason\":\"Wrong billing address\",\"billing_address\":$ADDRESS}")
ok "voided $(printf '%s' "$REISSUED" | jx .voided.number), reissued $(printf '%s' "$REISSUED" | jx .reissued.number)"
expect 409 POST "/api/admin/invoices/$INVOICE_ID/void-reissue" '{"reason":"again"}'
api GET "/api/admin/orders/$ORDER" >/dev/null; ok "admin order page"

step "admin: refund the duplicate charge (pre-approved)"
REFUND=$(api POST "/api/admin/orders/$ORDER/refunds" "{\"idempotency_key\":\"$(uuid)\",\"reason\":\"duplicate_charge\",\"charge_payment_id\":\"$DUPLICATE\"}" | jx .refund_id)
poll 60 GET "/api/admin/refunds/$REFUND" .refund.status completed
eq "$(api GET /api/admin/payments | jx '.duplicates[0].resolved')" true "duplicate resolved"

step "refund approval: human task, decide, refund done"
SCENARIO=$(api POST /api/demo/scenarios "{\"idempotency_key\":\"$(uuid)\",\"scenario\":\"delivered\"}")
ORDER2=$(printf '%s' "$SCENARIO" | jx .order.number)
ok "scenario order $ORDER2"
# A returned parcel starts a refund that waits for an admin's approval.
api POST "/api/orders/$ORDER2/demo/actions" '{"action":"request_return"}' >/dev/null; ok "return requested"
eq "$(api POST "/api/orders/$ORDER2/demo/actions" '{"action":"mark_returned"}' | jx .order.status)" returned "status"
poll 60 GET "/api/admin/tasks?kind=approve_refund" '.items[0].kind' approve_refund
TASK=$(api GET "/api/admin/tasks?kind=approve_refund" | jx '.items[0].id')
expect 422 POST "/api/admin/tasks/$TASK/decide" '{"decision":{"kind":"send"}}'
eq "$(api POST "/api/admin/tasks/$TASK/decide" '{"decision":{"kind":"approve"},"note":"smoke"}' | jx .task.status)" decided "task"
expect 409 POST "/api/admin/tasks/$TASK/decide" '{"decision":{"kind":"reject","reason":"changed my mind"}}'
poll 60 GET "/api/orders/$ORDER2" .order.status refunded
REFUND2=$(api GET "/api/admin/orders/$ORDER2" | jx '.refunds[0].id')
eq "$(api GET "/api/admin/refunds/$REFUND2" | jx .refund.status)" completed "approved refund"

step "admin: settings, inventory, customers, emails, tickets"
SETTINGS=$(api GET /api/admin/settings | jx .settings)
api PUT /api/admin/settings "{\"settings\":$SETTINGS}" >/dev/null; ok "settings saved"
api GET /api/admin/inventory >/dev/null; ok "inventory"
eq "$(api PUT "/api/admin/inventory/$PRODUCT" '{"stock":42}' | jx .stock.stock)" 42 "stock set"
eq "$(api GET /api/admin/customers | jx '.items[0].customer.email')" "smoke+$RUN@example.com" "customers"
api GET /api/admin/emails >/dev/null; ok "email log"
api GET /api/admin/tickets >/dev/null; ok "tickets"
api GET /api/admin/refunds >/dev/null; ok "refunds"
api GET /api/admin/investigations >/dev/null; ok "investigations"

step "admin: details, filters, cancel, investigation"
api GET "/api/admin/tasks/$TASK" >/dev/null; ok "task detail"
api GET "/api/admin/orders?q=$ORDER" >/dev/null; ok "order search"
USER_ID=$(printf '%s' "$ME" | jx .user.id)
api GET "/api/admin/customers/$USER_ID" >/dev/null; ok "customer detail"
EMAIL_ID=$(api GET /api/admin/emails | jx '.items[0].id')
api GET "/api/admin/emails/$EMAIL_ID" >/dev/null; ok "email detail"
api GET "/api/admin/payments?suspicious=true" >/dev/null; ok "suspicious payments"
api GET "/api/admin/inventory?low_only=true" >/dev/null; ok "low stock"
api POST /api/compat/alternatives "{\"product_ids\":[\"$PRODUCT\"],\"category\":\"cpu\"}" >/dev/null; ok "alternatives"
UNPAID=$(api POST /api/demo/scenarios "{\"idempotency_key\":\"$(uuid)\",\"scenario\":\"unpaid\"}" | jx .order.number)
eq "$(api POST "/api/admin/orders/$UNPAID/cancel" '{"reason":"smoke"}' | jx .order.status)" cancelled "cancelled $UNPAID"
expect 409 POST "/api/admin/orders/$ORDER/cancel" '{}'
SHIPPED=$(api POST /api/demo/scenarios "{\"idempotency_key\":\"$(uuid)\",\"scenario\":\"shipped_last_friday\"}" | jx .order.number)
call POST "/api/admin/orders/$SHIPPED/investigations" "{\"idempotency_key\":\"$(uuid)\"}"
eq "$STATUS" 200 "investigation on $SHIPPED"
expect 404 POST /api/admin/tickets/T-AAAAAA/actions '{"action":{"kind":"close"}}'
TOKEN_SAVE=$TOKEN; TOKEN=""
ETAG=$(curl -s -D - -o /dev/null "$API/api/catalog" | tr -d '\r' | awk -F': ' 'tolower($1)=="etag"{print $2}')
eq "$(curl -s -o /dev/null -w '%{http_code}' -H "if-none-match: $ETAG" "$API/api/catalog")" 304 "catalog revalidation"
TOKEN=$TOKEN_SAVE

step "support form"
eq "$(api GET /api/tickets | jx '.items | length')" 0 "tickets"
call POST /api/support/messages "{\"idempotency_key\":\"$(uuid)\",\"subject\":\"Where is my order?\",\"body\":\"Hi, where is $ORDER?\"}"
eq "$STATUS" 200 "support message"
TICKET=$(printf '%s' "$BODY" | jx .ticket_id)
poll 60 GET "/api/tickets/$TICKET" .ticket.id "$TICKET"

step "access control"
MEMBER=$(TOKEN="" api POST /api/dev/login "{\"email\":\"member+$RUN@example.com\",\"org_name\":\"Smoke $RUN\",\"role\":\"org:member\"}" | jx .token)
ADMIN_TOKEN=$TOKEN
TOKEN=$MEMBER
expect 403 GET /api/admin/overview
expect 404 GET "/api/orders/$ORDER"
TOKEN=$ADMIN_TOKEN
expect 404 GET /api/no-such-route

step "demo reset"
eq "$(api POST /api/admin/demo/reset '{"confirm":"RESET"}' | jx .orders)" 4 "orders deleted"
eq "$(api GET /api/orders | jx '.items | length')" 0 "orders after reset"

printf '\n\033[32mSmoke test passed.\033[0m\n'
