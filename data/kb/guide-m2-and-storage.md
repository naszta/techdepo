---
slug: guide-m2-and-storage
title: "Buying guide: M.2, NVMe and SATA storage"
summary: NVMe vs SATA, M.2 slots and keys, PCIe generations, how many drives a board takes, and which drive to choose.
---

# M.2, NVMe and SATA storage

## The types of drive

| Type | Connects to | Speed | Example |
|---|---|---|---|
| NVMe SSD (M.2 2280) | M.2 slot | 3,500–14,500 MB/s | Arcwave A3 500 GB and 1 TB, Arcwave A4 1 TB and 2 TB, Arcwave A5 2 TB and 4 TB, Lodestar Q3 2 TB, Lodestar Q4 4 TB |
| M.2 SATA SSD | M.2 slot that supports SATA | ~550 MB/s | Lodestar Glide M.2 SATA 1 TB |
| 2.5-inch SATA SSD | SATA port + bay | ~550 MB/s | Arcwave S2 1 TB and 2 TB |
| 3.5-inch hard drive | SATA port + 3.5-inch bay | ~190 MB/s | Lodestar Vault 4 TB |

## M.2 slots

- **Count the M-key slots.** Each M.2 drive needs its own. Boards also have an **E-key** slot
  for the Wi-Fi module, which never takes a drive. The Meridian A520M Core, Meridian A620M Lite
  and Meridian H610M Base have one M-key slot; the others have two to five.
- **NVMe or SATA?** Every M-key slot takes NVMe drives. Only some also take **M.2 SATA** drives;
  each board's page says which. The Meridian A620M Lite, Meridian H610M Base, Kestrel B650E-I Perch
  and Kestrel B860I Hover have NVMe-only slots.
- **PCIe generation.** Drives and slots are backwards compatible: a PCIe 5.0 drive works in a 4.0
  slot at 4.0 speed, and a 3.0 drive in a 5.0 slot at its own speed. PCIe 5.0 drives only reach
  full speed in a PCIe 5.0 slot.

## SATA ports

Each 2.5-inch or 3.5-inch drive uses one SATA port and one SATA power connector. Mini-ITX boards
often have only two ports (Kestrel B650E-I Perch, Kestrel B860I Hover), so a third SATA drive does
not fit.

The PC builder counts M.2 drives against M.2 slots and SATA drives against SATA ports.

## Which drive should I buy?

- **Boot drive:** any NVMe SSD. 1 TB is the comfortable size.
- **Games:** PCIe 4.0 drives load games as fast as PCIe 5.0 in practice. QLC drives (Lodestar)
  give lots of capacity cheaply; they slow down only during very long writes.
- **Video and large files:** a PCIe 5.0 drive in a PCIe 5.0 slot, or a TLC drive such as the
  Arcwave A4 2 TB.
- **Backups and archives:** a 3.5-inch hard drive, if your case has a 3.5-inch bay.
