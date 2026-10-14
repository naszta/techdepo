## Overview

The Nimbus Stratus DDR5-6000 64 GB kit has four 16 GB modules rated for 6000 MT/s at CL30, with
low 34 mm heat spreaders. It fills all four slots of an ATX or Micro-ATX DDR5 board.

Two caveats. First, it needs a board with four memory slots: Mini-ITX boards such as the Kestrel
B650E-I Perch and Kestrel B860I Hover, and the two-slot Meridian A620M Lite, cannot take it.
Second, four modules mean two per memory channel, and DDR5 CPUs officially support much lower
speeds that way: 3600 MT/s on AM5 and LGA1700 CPUs and 4400 MT/s on Osprey (LGA1851) CPUs. The kit
runs at that speed by default, and the 6000 MT/s profile is much less likely to be stable than
with a two-module kit. For 64 GB on any board, the two-module Lumen Pulse DDR5-6400 64 GB is the
alternative.

## Who it's for

Workstation builders with four-slot DDR5 boards who want 64 GB of matched memory.

## Pros

- 64 GB as a matched set
- Low-profile 34 mm modules clear any cooler
- CL30 timings

## Cons

- Needs four memory slots
- Runs at 3600 MT/s (AM5, LGA1700) or 4400 MT/s (LGA1851) by default with four modules
- No free slots for later upgrades

## FAQ

### Will it work in the Kestrel B650E-I Perch?

No. Mini-ITX boards have two slots. Choose the Lumen Pulse DDR5-6400 64 GB (2x32 GB) instead.

### Why does it run slower than 6000 MT/s?

Four DDR5 modules are much harder to drive than two. With two modules per channel, CPUs officially
support 3600 MT/s (AM5 and LGA1700) or 4400 MT/s (LGA1851), and the kit runs at that speed by
default. The 6000 MT/s profile may need extra tuning or may not be stable on every board.

### Two modules or four?

For DDR5, two larger modules are usually the better choice.
