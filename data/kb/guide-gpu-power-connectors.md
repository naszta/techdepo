---
slug: guide-gpu-power-connectors
title: "Buying guide: graphics card power connectors"
summary: Slot power, 8-pin PCIe and 12V-2x6 connectors, how to check that a power supply can feed your card, and common mismatches.
---

# Graphics card power connectors

## Three ways a card gets power

- **The PCIe slot** supplies up to 75 W. Low-power cards such as the Aurex Nova 50 (70 W) need
  nothing else.
- **8-pin PCIe connectors** supply up to 150 W each. Cards use one to three of them.
- **12V-2x6** (also called 16-pin) is a single connector for up to 600 W. Newer Halcyon cards (HX
  7070, HX 7080, HX 7090) use it.

## Does my power supply have the right connectors?

The power supply must have **at least as many** of each connector as the card needs. Our cards do
not include adapters, so the PC builder checks for native cables.

| Card | Needs | Power supplies that have it |
|---|---|---|
| Halcyon HX 6050 Solo, HX 6060 Dual, Aurex Nova 60 | 1 × 8-pin | all |
| Halcyon HX 6070 Dual, Aurex Nova 70 | 2 × 8-pin | all |
| Aurex Nova 80, Nova 90 | 3 × 8-pin | Ferro Core 750, Nordwatt Prime 750 and larger |
| Halcyon HX 7070 Twin, HX 7070 SFF, HX 7080 Triple, HX 7090 Triple | 1 × 12V-2x6 | Nordwatt Prime 650, 750, 850 and 1000, Nordwatt Titan 1200, Ferro SF750, Ferro SF850L |

## Common mismatches

- **Halcyon HX 7080 Triple + Ferro Core 650:** the Core 650 has no 12V-2x6 connector. Choose a unit with a native 12V-2x6 cable, such as the Nordwatt Prime 750.
- **Aurex Nova 80 + Ferro Core 650:** the card needs three 8-pin connectors and the PSU has two.
  The Ferro Core 750 has four.
- **Halcyon HX 7070 SFF + Ferro SF600:** no 12V-2x6 on the SF600. The Ferro SF750 has it and is
  the same size.

## Connecting a 12V-2x6 cable

Push the connector in until it clicks and sits fully flush, with no gap. Avoid bending the cable
sharply right at the connector; leave about 3 cm straight first. A partly inserted 16-pin
connector can overheat.

## Wattage still matters

The right connectors do not mean enough power. See "Power supply sizing and headroom".
