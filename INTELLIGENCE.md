# Business Intelligence — definitions

The eight intelligence areas are one reporting standard shared by SME Pro and
Pharmacy Pro. This file is the reference for what each term means. SME Pro's
implementation is `src-tauri/src/intelligence.rs`, exposed at
`GET /reports/intelligence?days=` and shown in **Reports → Business
intelligence**. It is read-only: it writes nothing and adds no tables.

## An area counts as implemented only when

it analyses the stored records, produces findings the data actually supports,
presents them plainly, and shows the evidence. Stored data or an existing
report is not enough. Where essential information isn't recorded, the area
says what is missing (`missing`) and produces only what the data supports.
Nothing is estimated to fill a gap.

## The insight

Every finding has: `what_happened`, `evidence`, `period`, `comparison`
(against the equal period before it), `why_it_matters`, `impact` (amount and
the basis it is measured on), `recommended_action`, `confidence` (level plus
stated limitations) and `follow_up`.

- **Severity:** `problem`, `watch`, `info`, `good`.
- **Confidence:** starts `high`. Any stated limitation makes it `medium`.
  Thin coverage or too little history makes it `low`. Fewer than 20 records
  adds a limitation of its own.
- **Follow-up** is a stated check for the next period. Whether a past finding
  later cleared is not tracked, because that would need stored snapshots.

## Periods

`days` (7–180, default 30) ending today is the **current period**; the equal
number of days before it is the **previous period**; the same again before
that is used for growth. A comparison is made only when recorded history
fully covers the previous period. Otherwise the area says how much history
exists and gives no comparison.

## Measures (identical in both apps)

| Measure | Definition |
|---|---|
| Revenue | Sum of sale revenue, net of refunds, by sale date |
| Gross profit | Revenue minus recorded cost of sales |
| Gross margin | Gross profit ÷ revenue |
| Cost data coverage | Revenue on sale lines that carry a recorded cost ÷ revenue |
| Order | Distinct order, or the sale line when it has no order id |
| Operating expenses | Recorded expenses other than those the app posts itself (purchases received, refunds, debt settlement, repack rounding), recognised by category and the wording the app gives them, so an entry a person types under "Purchasing" still counts |
| Cash in / out | Income / expense entries by transaction date (cash basis) |
| Identified customer | A sale with a customer phone, otherwise a customer name |
| New customer / product | First ever sale falls inside the period |
| Lapsed customer | Bought in the previous period, not in the current one |

Profit measures are shown or compared only when at least 80% of revenue in the
period has a recorded cost.

## Rules that fire (SME Pro thresholds)

| Area | Finding | Fires when |
|---|---|---|
| Sales | Sales change | Revenue differs ≥15% from previous period (≥10 sale lines in the two periods); problem at ≤ −30% |
| Sales | Product decline | A product with ≥3% of last period's revenue lost ≥40% |
| Sales | Concentration | ≥4 products sold and top product ≥40%, or top three ≥70% |
| Profitability | Cost data gap | Coverage < 80% (problem < 50%) |
| Profitability | Margin shift | Margin moved ≥3 points, coverage ≥80% in both periods; problem at ≤ −6 |
| Profitability | Items below cost | An item with recorded cost has negative profit |
| Profitability | Profit concentration | ≥4 profitable items and the top one ≥50% |
| Profitability | Expenses vs profit | Recorded operating expenses ≥70% of gross profit |
| Inventory | Sold-out sellers | Stock ≤0 on an item that sold in the period |
| Inventory | Dead stock | No sale in 60 days (existing slow-movers logic); problem at ≥25% of stock value |
| Inventory | Stock-out risk | Under 7 days of stock at 30-day pace (existing runway logic) |
| Inventory | Expiry risk | Expired or expiring-within-30-days batches (existing expiry logic) |
| Customer | Concentration | ≥4 customers and top ≥30%, or top three ≥60% |
| Customer | Lapsed customers | ≥3 lapsed, and ≥10% of last period's identified revenue |
| Customer | Retention | ≥5 customers last period; under 50% returned (watch) or ≥75% (good) |
| Expense | Expense change | Operating expenses up ≥20% or down ≥30% (≥3 entries) |
| Expense | Expense ratio | Expenses as a share of sales up ≥5 points |
| Expense | Concentration | ≥3 categories and top ≥50% |
| Expense | Unusual expense | An entry ≥3× the median of the last 180 days (≥8 entries) |
| Expense | Recording gap | Sales continue, no operating expense in 90 days |
| Cash flow | Cash flow trend | ≥3 entries; net negative (problem if also negative before), or net up ≥20% |
| Cash flow | Customer debts overdue | Any overdue receivable (existing aging logic) |
| Cash flow | Payments due | Debts you owe overdue or due in 14 days, and unmet by the period's net cash |
| Performance | Scorecard | ≥3 comparable measures; each counts as improved or worsened at ±5% (±1 point for rates) |
| Performance | Refund share | Refunds up ≥2 points as a share of sales |
| Growth | Revenue growth | Two consecutive periods both ≥ +5% or both ≤ −5%, or a direction change ≥10% |
| Growth | Growing / new products | +30% on a product worth ≥3% of sales; new products ≥2% of revenue |
| Growth | Customer growth | New customers up or down ≥30% (≥3 in each period) |
| Growth | Revenue outlook | Three covered periods; average with low–high range |

## What the apps don't record (stated in the report, never guessed)

- **Targets and budgets** — Performance compares with the previous period only.
- **Opening cash or bank balance** — Cash Flow analyses movement, not position
  or runway.
- **Growth drivers** (marketing spend, capacity) — Growth is described from
  sales, customers and products.
- **Customer identity on every sale** — customer findings cover identified
  sales only, and the share covered is shown.
- **Stock-to-sales link by ID** — SME Pro matches stock and sales by item
  name, so a renamed item loses its history (stated as a limitation).
