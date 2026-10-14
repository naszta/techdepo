---
slug: guide-psu-sizing
title: "Buying guide: power supply sizing and headroom"
summary: How to choose PSU wattage from CPU and graphics card power, why 20 % headroom matters, and how the PC builder estimates load.
---

# Power supply sizing and headroom

## What uses the power?

Two parts dominate a PC's power draw:

- **The CPU.** Use its peak (sustained) power, not the TDP. The Helion H7 1470 has a 125 W TDP
  but draws up to about 253 W under full load. The Voltaris V5 8600 peaks around 88 W.
- **The graphics card.** Use its board power: from 70 W (Aurex Nova 50) to 450 W (Halcyon HX
  7090 Triple).

The motherboard, memory, drives and fans add a smaller amount on top.

## How the PC builder estimates load

The PC builder adds up the power of every part in your build and adds 10 % as a safety margin.
Then it compares the result with the power supply's wattage:

- **Error:** the estimate, including the 10 % margin, is above the PSU's wattage, so there is no
  safety margin left. Choose a bigger power supply.
- **Warning:** less than 20 % headroom is left. It works, but the PSU runs hot and loud, and
  power spikes from modern graphics cards can trip its protection.
- **OK:** 20 % headroom or more.

## Rules of thumb

| Build | Example | Suggested PSU |
|---|---|---|
| Office, integrated graphics | Voltaris V5 5600G | 550 W (our smallest) |
| 65 W CPU + card up to ~200 W | V5 8600 + Halcyon HX 6060 Dual | 550–650 W |
| 65–120 W CPU + 220–260 W card | V7 8700 + Aurex Nova 70 | 750 W |
| High-power CPU + 320–355 W card | H7 1470 + Halcyon HX 7080 Triple | 1000 W |
| Flagship card (450 W) | any CPU + Halcyon HX 7090 Triple | 1000–1200 W |

The graphics card vendor's recommended PSU wattage, shown on each card's page, assumes a typical
system. With a high-power CPU, go one step higher.

## Typical traps

- **Ferro Core 550 + Helion H7 1470 + Aurex Nova 70:** about 516 W from CPU and card alone. With
  the rest of the system and the margin, it is above 550 W: error.
- **Nordwatt Prime 650 + Voltaris V9 8950 + Halcyon HX 7070 Twin:** it fits, but headroom drops below
  20 %: warning. The Nordwatt Prime 850 fixes it.

## Wattage is not everything

The PSU also needs the right **connectors** for the graphics card (see "GPU power connectors")
and must **fit the case**: ATX or SFX, and not longer than the case allows (see "Form factors and
case clearance").

## Efficiency ratings

80 PLUS Bronze, Gold, Platinum and Titanium describe how much power is lost as heat. Higher
ratings run cooler and quieter and save a little electricity. They do not change how much power
the PSU can deliver.
