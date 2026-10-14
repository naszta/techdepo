---
slug: guide-bios-updates
title: "Buying guide: BIOS updates and BIOS flashback"
summary: Why a new CPU can need a BIOS update on an older board, how to tell, and how BIOS flashback lets you update without a CPU.
---

# BIOS updates and BIOS flashback

## Why would a board need an update?

A motherboard's BIOS (firmware) must know a CPU family to start it. When a socket gets a new CPU
generation, boards made earlier ship with a BIOS that predates it. They support the new CPUs, but
only after a BIOS update. Without it, the PC does not start or shows no picture.

On our platforms this concerns:

- **AM5:** Obsidian CPUs (Voltaris V5 8600, V7 8700, V9 8950) on boards built for the first
  generation (Quartz).
- **LGA1700:** Harrier CPUs (Helion H5 1440F, H7 1470) on boards built for the first generation
  (Falcon).

## How do I know?

Each board's page lists the BIOS version it ships with and the minimum BIOS each CPU family needs.
The PC builder compares them and warns when an update is needed. Examples:

- **Kestrel B650M Wing:** ships with BIOS 10; Obsidian needs BIOS 16. Update needed, and the
  board has BIOS flashback.
- **Meridian A620M Lite:** ships with BIOS 8; Obsidian needs BIOS 15. Update needed, and there is
  **no** flashback.
- **Kestrel B650 Aero:** ships with BIOS 22, so Obsidian works out of the box.

## BIOS flashback: update without a CPU

Boards with BIOS flashback have a button and a marked USB port on the back panel:

1. Download the latest BIOS for your board from the manufacturer's website on another computer.
2. Copy it to a USB stick (FAT32), renamed as the board's manual says.
3. Connect the board to the power supply, plug the stick into the flashback port and press the
   button. The light blinks during the update; do not switch off.
4. When it stops, install the new CPU and start the PC.

## No flashback?

The board needs a CPU it already supports to run the update. Install an older CPU of the same
socket (for AM5, a Quartz CPU such as the Voltaris V5 7600), update the BIOS from the BIOS menu,
then swap in the new CPU. If you do not have one, choose a board with flashback or one that ships
with a new enough BIOS.

## When no update helps

A board supports only the CPU families its maker released a BIOS for. If a family is missing from
the board's list, no BIOS version adds it, and the PC builder marks the pair as incompatible
rather than warning about an update. On our platforms this concerns the **Meridian H610M Base**,
which runs Falcon CPUs (Helion H3 1210) only: Harrier CPUs need another LGA1700 board, such as the
Kestrel B760 Vector or the Meridian B760M Forge D4.

## Is a BIOS update risky?

Very rarely. Do not switch off the PC during the update. If a board does not start after a CPU
upgrade, a missing BIOS update is the most common cause; contact support with your order number.
