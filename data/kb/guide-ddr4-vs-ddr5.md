---
slug: guide-ddr4-vs-ddr5
title: "Buying guide: DDR4 vs DDR5 memory"
summary: Which platforms use DDR4 or DDR5, why they are not interchangeable, memory speeds and profiles, and how many modules to buy.
---

# DDR4 vs DDR5 memory

## They are not interchangeable

DDR4 and DDR5 modules have the notch in a different place, so a DDR4 module physically does not fit
a DDR5 slot, and the other way round. The motherboard decides which type you need.

| Platform | Memory | Our boards |
|---|---|---|
| AM4 | DDR4 only | Meridian A520M Core, Kestrel B550 Glide |
| AM5 | DDR5 only | Meridian A620M Lite, Kestrel B650M Wing, Kestrel B650 Aero, Kestrel B650E-I Perch, Meridian X870E Crest |
| LGA1700 | DDR4 **or** DDR5, depending on the board | Meridian B760M Forge D4 (DDR4); Meridian H610M Base, Kestrel B760 Vector, Meridian Z790 Crest (DDR5) |
| LGA1851 | DDR5 only | Kestrel B860I Hover, Meridian Z890 Crest |

LGA1700 CPUs support both types, but each board has slots for only one. A "D4" in the board's
name usually means DDR4.

A common question: "Will the Kestrel B650 work with my Nimbus DDR4 kit?" No. Every Kestrel B650
board is AM5 and takes DDR5 only. Keep the DDR4 kit for an AM4 or DDR4 LGA1700 build, or buy a
DDR5 kit.

## Speed: rated, supported and actual

A kit's rated speed (for example DDR5-6000) is reached with its XMP or EXPO profile. Each CPU has
an official speed, and each board a maximum:

| CPU family | Official speed, two modules | Official speed, four modules |
|---|---|---|
| Granite (AM4) | DDR4-3200 | DDR4-2667 |
| Quartz (AM5, 1st gen) | DDR5-5200 | DDR5-3600 |
| Obsidian (AM5, 2nd gen) | DDR5-5600 | DDR5-3600 |
| Falcon (LGA1700) | DDR5-4800 / DDR4-3200 | DDR5-3600 / DDR4-2933 |
| Harrier (LGA1700) | DDR5-5600 / DDR4-3200 | DDR5-3600 / DDR4-2933 |
| Osprey (LGA1851) | DDR5-6400 | DDR5-4400 |

Every CPU we sell has two memory channels. Four modules put two on each channel, and the official
speed drops, sharply for DDR5.

A faster kit still works, but runs at the CPU's official speed by default. Enabling the profile
runs it faster; this is a memory overclock that the board must be rated for. The PC builder shows
a note when a kit is faster than the platform supports. It is information, not a problem.

## How many modules?

- Buy **two modules** for dual-channel bandwidth.
- **Mini-ITX boards and some budget boards have only two slots**, so a four-module kit cannot be
  installed. Check the slot count.
- Check the board's **maximum capacity**: the Meridian A620M Lite supports 64 GB, so a 96 GB kit
  does not fit.
- With DDR5, two larger modules run faster and more reliably than four smaller ones: four modules
  officially run at 3600 MT/s on AM5 and LGA1700 and 4400 MT/s on LGA1851. For 64 GB, the
  two-module Lumen Pulse DDR5-6400 64 GB (2x32 GB) is the better choice than four 16 GB modules.

## How much?

16 GB for office PCs, 32 GB for gaming, 64 GB or more for VMs, large projects and video work.
