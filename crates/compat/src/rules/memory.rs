//! Memory: DDR generation, slots, capacity, speed, mixed kits.

use techdepo_catalog::MemoryType;
use techdepo_catalog::spec::{MEMORY_CHANNELS, MemorySpec};

use super::{CHECKED, Checked, Ctx, count, distinct, join_and, names};
use crate::build::Part;
use crate::report::Issue;
use crate::rule::RuleId;

pub(super) fn memory_type(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let kits = cx.memory();
    match cx.motherboard() {
        Some(board) => {
            if kits.is_empty() {
                return None;
            }
            let slots = board.spec.memory_type;
            for kit in kits.iter().filter(|k| k.spec.memory_type != slots) {
                out.push(Issue::new(
                    RuleId::MemoryType,
                    [kit.id(), board.id()],
                    format!(
                        "{kit} is {} memory, but {board} has {slots} slots. {} and {slots} \
                         modules are keyed differently and won't physically fit.",
                        kit.spec.memory_type, kit.spec.memory_type,
                    ),
                ));
            }
        }
        None => {
            if kits.len() < 2 {
                return None;
            }
            let of = |t: MemoryType| kits.iter().filter(move |k| k.spec.memory_type == t);
            let (ddr4, ddr5): (Vec<_>, Vec<_>) = (
                of(MemoryType::Ddr4).collect(),
                of(MemoryType::Ddr5).collect(),
            );
            if !ddr4.is_empty() && !ddr5.is_empty() {
                out.push(Issue::new(
                    RuleId::MemoryType,
                    kits.iter().map(|k| k.id()),
                    format!(
                        "The build mixes DDR4 ({}) and DDR5 ({}) memory. A motherboard takes \
                         only one memory generation.",
                        names(ddr4),
                        names(ddr5),
                    ),
                ));
            }
        }
    }
    CHECKED
}

/// With a board, the CPU must support the board's memory type (kits are checked against the
/// board by `memory_type`). Without one, the CPU must support each kit.
pub(super) fn cpu_memory_type(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let cpu = cx.cpu()?;
    let supported = || join_and(cpu.spec.memory.iter().map(|m| m.memory_type));
    match cx.motherboard() {
        Some(board) => {
            let slots = board.spec.memory_type;
            if cx.same_platform() && !cpu.spec.supports_memory(slots) {
                out.push(Issue::new(
                    RuleId::CpuMemoryType,
                    [cpu.id(), board.id()],
                    format!(
                        "{board} takes {slots} memory, but {cpu}'s memory controller only \
                         supports {}.",
                        supported(),
                    ),
                ));
            }
        }
        None => {
            if cx.memory().is_empty() {
                return None;
            }
            for kit in cx.memory() {
                if !cpu.spec.supports_memory(kit.spec.memory_type) {
                    out.push(Issue::new(
                        RuleId::CpuMemoryType,
                        [kit.id(), cpu.id()],
                        format!(
                            "{kit} is {} memory, but {cpu}'s memory controller only supports {}.",
                            kit.spec.memory_type,
                            supported(),
                        ),
                    ));
                }
            }
        }
    }
    CHECKED
}

pub(super) fn memory_slots(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let board = cx.motherboard()?;
    let kits = non_empty(cx.memory())?;
    let modules: u32 = kits.iter().map(|k| u32::from(k.spec.modules)).sum();
    let slots = board.spec.memory_slots;
    if modules > u32::from(slots) {
        let what = match kits {
            [kit] => format!("{kit} has {}", count(kit.spec.modules, "module", "modules")),
            _ => format!("{} have {modules} modules in total", names(kits)),
        };
        out.push(Issue::new(
            RuleId::MemorySlots,
            std::iter::once(board.id()).chain(kits.iter().map(|k| k.id())),
            format!(
                "{what}, but {board} has only {}.",
                count(slots, "memory slot", "memory slots")
            ),
        ));
    }
    CHECKED
}

pub(super) fn memory_capacity(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let board = cx.motherboard()?;
    let kits = non_empty(cx.memory())?;
    let total_gb: u32 = kits.iter().map(|k| k.spec.total_gb()).sum();
    let max_gb = board.spec.max_memory_gb;
    if total_gb > u32::from(max_gb) {
        let what = match kits {
            [kit] => format!("{kit} is {total_gb} GB"),
            _ => format!("{} add up to {total_gb} GB", names(kits)),
        };
        out.push(Issue::new(
            RuleId::MemoryCapacity,
            std::iter::once(board.id()).chain(kits.iter().map(|k| k.id())),
            format!("{what}, but {board} supports at most {max_gb} GB."),
        ));
    }
    CHECKED
}

/// A kit rated faster than the CPU's memory controller or the board supports runs at the
/// lower speed by default. More modules than channels (two per channel) lower the CPU's
/// official speed further. Identical kits share one note.
///
/// Quiet for a kit that does not fit the board, for kits of different generations and for more
/// modules than the board has slots: `memory_type` and `memory_slots` report those as errors.
pub(super) fn memory_speed(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let kits = non_empty(cx.memory())?;
    if cx.cpu().is_none() && cx.motherboard().is_none() {
        return None;
    }
    let modules: u32 = kits.iter().map(|k| u32::from(k.spec.modules)).sum();
    let overfull = cx
        .motherboard()
        .is_some_and(|b| modules > u32::from(b.spec.memory_slots));
    if mixes_generations(kits) || overfull {
        // `memory_type` or `memory_slots` reports an error; a speed note would be noise.
        return CHECKED;
    }
    let two_per_channel = modules > MEMORY_CHANNELS;
    for group in distinct(kits) {
        let kit = group[0];
        let memory_type = kit.spec.memory_type;
        if cx
            .motherboard()
            .is_some_and(|b| b.spec.memory_type != memory_type)
        {
            continue;
        }
        // The lowest applicable limit; the CPU wins ties (the memory controller sets the pace).
        let cpu_limit = cx.cpu().and_then(|cpu| {
            let mts = cpu.spec.max_memory_speed_mts(memory_type, modules)?;
            let why = if two_per_channel {
                format!(
                    "with {modules} modules installed (two per channel) {cpu} officially supports \
                     {memory_type} up to"
                )
            } else {
                format!("{cpu} officially supports {memory_type} up to")
            };
            Some((mts, cpu.id(), why))
        });
        let board_limit = cx.motherboard().map(|b| {
            let mts = b.spec.max_memory_speed_mts;
            (mts, b.id(), format!("{b} supports memory up to"))
        });
        // Without a CPU, the board's limit says nothing about two modules per channel, where the
        // CPU's official speed is far lower on every platform.
        if two_per_channel && cx.cpu().is_none() {
            if let Some(board) = cx.motherboard() {
                let verb = if group.len() == 1 { "is" } else { "are" };
                out.push(Issue::new(
                    RuleId::MemorySpeed,
                    [kit.id(), board.id()],
                    format!(
                        "{} {verb} rated for {} MT/s, but with {modules} modules installed in \
                         {board} (two per channel) the memory runs at the CPU's official speed \
                         for that layout, well below the rated speed. Choose a CPU to see the \
                         exact speed.",
                        names(group.iter().copied()),
                        kit.spec.speed_mts,
                    ),
                ));
            }
            continue;
        }
        let limit = match (cpu_limit, board_limit) {
            (Some(c), Some(b)) => Some(if b.0 < c.0 { b } else { c }),
            (c, b) => c.or(b),
        };
        let Some((limit_mts, limiter, why)) = limit else {
            continue;
        };
        if kit.spec.speed_mts > limit_mts {
            let verb = if group.len() == 1 { "is" } else { "are" };
            out.push(Issue::new(
                RuleId::MemorySpeed,
                [kit.id(), limiter],
                format!(
                    "{} {verb} rated for {} MT/s, but {why} {limit_mts} MT/s, so the memory will \
                     run at {limit_mts} MT/s. Its EXPO/XMP profile may reach a higher speed, but \
                     that isn't guaranteed.",
                    names(group.iter().copied()),
                    kit.spec.speed_mts,
                ),
            ));
        }
    }
    CHECKED
}

/// Several kits run at the slowest kit's settings. Quiet when the kits mix generations or do
/// not fit the board: `memory_type` reports that as an error.
pub(super) fn memory_mixed_kits(cx: &Ctx<'_, '_>, out: &mut Vec<Issue>) -> Checked {
    let kits = cx.memory();
    if kits.len() < 2 {
        return None;
    }
    let wrong_board = cx.motherboard().is_some_and(|b| {
        kits.iter()
            .any(|k| k.spec.memory_type != b.spec.memory_type)
    });
    if mixes_generations(kits) || wrong_board {
        return CHECKED;
    }
    let slowest = kits
        .iter()
        .map(|k| k.spec.speed_mts)
        .min()
        .unwrap_or_default();
    let loosest = kits
        .iter()
        .map(|k| k.spec.cas_latency)
        .max()
        .unwrap_or_default();
    let identical = kits.iter().all(|k| k.id() == kits[0].id());
    let detail = if identical {
        format!(
            "{} are separate kits that aren't tested as one set",
            names(kits)
        )
    } else {
        format!(
            "{} are separate kits that aren't tested together, so all modules run at the \
             slowest settings ({slowest} MT/s, CL{loosest})",
            names(kits),
        )
    };
    out.push(Issue::new(
        RuleId::MemoryMixedKits,
        kits.iter().map(|k| k.id()),
        format!(
            "{detail}. Mixed kits usually work, but a single kit with the same total capacity \
             is the safer choice."
        ),
    ));
    CHECKED
}

/// Kits of more than one DDR generation.
fn mixes_generations(kits: &[Part<'_, MemorySpec>]) -> bool {
    kits.iter()
        .any(|k| k.spec.memory_type != kits[0].spec.memory_type)
}

fn non_empty<T>(items: &[T]) -> Option<&[T]> {
    (!items.is_empty()).then_some(items)
}

#[cfg(test)]
mod tests {
    use techdepo_catalog::spec::MemorySupport;
    use techdepo_catalog::{CpuFamily, MemoryType, Socket};

    use crate::fixtures::*;
    use crate::rule::{RuleId, Severity};

    fn ddr4_kit(id: &str) -> techdepo_catalog::Product {
        memory_product(id, |s| {
            s.memory_type = MemoryType::Ddr4;
            s.speed_mts = 3200;
            s.cas_latency = 16;
        })
    }

    #[test]
    fn kit_must_match_board_generation() {
        let (kit, board) = (ddr4_kit("ddr4"), board_product("b", |_| {}));
        let report = check(&[&kit, &board]);
        let issue = only_issue(&report, RuleId::MemoryType);
        assert_eq!(issue.severity, Severity::Error);
        assert!(issue.message.contains("DDR4") && issue.message.contains("DDR5 slots"));
        assert_eq!(fired(&report), [RuleId::MemoryType]);
    }

    #[test]
    fn kits_must_agree_without_a_board() {
        let report = check(&[&ddr4_kit("a"), &memory_product("b", |_| {})]);
        assert_eq!(fired(&report), [RuleId::MemoryType]);
        let same = check(&[&memory_product("a", |_| {}), &memory_product("b", |_| {})]);
        assert!(!fired(&same).contains(&RuleId::MemoryType));
    }

    #[test]
    fn cpu_controller_checked_against_kit_without_board() {
        let report = check(&[&cpu_product("c", |_| {}), &ddr4_kit("k")]);
        let issue = only_issue(&report, RuleId::CpuMemoryType);
        assert!(
            issue.message.contains("only supports DDR5"),
            "{}",
            issue.message
        );
    }

    #[test]
    fn cpu_controller_checked_against_board_when_present() {
        let cpu = cpu_product("c", |s| {
            s.socket = Socket::Lga1700;
            s.family = CpuFamily::Falcon;
        });
        let board = board_product("b", |s| {
            s.socket = Socket::Lga1700;
            s.chipset = techdepo_catalog::Chipset::B760;
            s.memory_type = MemoryType::Ddr4;
            s.max_memory_speed_mts = 3600;
            s.cpu_support[0].family = CpuFamily::Falcon;
            s.cpu_support[1].family = CpuFamily::Harrier;
        });
        let report = check(&[&cpu, &board, &ddr4_kit("k")]);
        assert_eq!(fired(&report), [RuleId::CpuMemoryType]);

        let both = cpu_product("c2", |s| {
            s.socket = Socket::Lga1700;
            s.family = CpuFamily::Falcon;
            s.memory.push(MemorySupport {
                memory_type: MemoryType::Ddr4,
                max_speed_mts: 3200,
                max_speed_2dpc_mts: 2933,
            });
        });
        assert!(check(&[&both, &board, &ddr4_kit("k")]).issues.is_empty());
    }

    #[test]
    fn too_many_modules() {
        let board = board_product("b", |s| s.memory_slots = 2);
        let kit = memory_product("k", |_| {});
        let report = check(&[&board, &kit, &kit]);
        let issue = only_issue(&report, RuleId::MemorySlots);
        assert!(
            issue.message.contains("2× k have 4 modules in total"),
            "{}",
            issue.message
        );
        assert!(
            issue.message.contains("only 2 memory slots"),
            "{}",
            issue.message
        );
        assert!(check(&[&board, &kit]).issues.is_empty());
    }

    #[test]
    fn too_much_capacity() {
        let board = board_product("b", |s| s.max_memory_gb = 64);
        let big = memory_product("k", |s| s.module_gb = 48);
        let issue = only_issue(&check(&[&board, &big]), RuleId::MemoryCapacity).clone();
        assert!(issue.message.contains("96 GB") && issue.message.contains("at most 64 GB"));
        let fits = memory_product("k", |s| s.module_gb = 32);
        assert!(check(&[&board, &fits]).issues.is_empty());
    }

    #[test]
    fn fast_kit_runs_at_the_lowest_limit() {
        let kit = memory_product("k", |s| s.speed_mts = 7200);
        let cpu = cpu_product("c", |_| {});
        let board = board_product("b", |_| {});
        let report = check(&[&cpu, &board, &kit]);
        let issue = only_issue(&report, RuleId::MemorySpeed);
        assert_eq!(issue.severity, Severity::Note);
        assert_eq!(issue.parts, [kit.id.clone(), cpu.id.clone()]);
        assert!(
            issue.message.contains("run at 5600 MT/s"),
            "{}",
            issue.message
        );
        assert_eq!(report.verdict, crate::Verdict::Compatible);

        let slow_board = board_product("b", |s| s.max_memory_speed_mts = 5200);
        let issue = only_issue(&check(&[&cpu, &slow_board, &kit]), RuleId::MemorySpeed).clone();
        assert_eq!(issue.parts[1], slow_board.id);
        assert!(issue.message.contains("run at 5200 MT/s"));

        let at_limit = memory_product("k", |s| s.speed_mts = 5600);
        assert!(check(&[&cpu, &board, &at_limit]).issues.is_empty());
    }

    #[test]
    fn four_modules_run_at_the_two_per_channel_speed() {
        let cpu = cpu_product("c", |_| {});
        let board = board_product("b", |_| {});
        let kit = memory_product("k", |s| s.speed_mts = 6000);
        // Two copies: one note naming both, at the CPU's two-per-channel speed.
        let report = check(&[&cpu, &board, &kit, &kit]);
        let issue = only_issue(&report, RuleId::MemorySpeed);
        assert_eq!(issue.parts, [kit.id.clone(), cpu.id.clone()]);
        assert!(
            issue.message.starts_with(
                "2× k are rated for 6000 MT/s, but with 4 modules installed (two per channel) c \
                 officially supports DDR5 up to 3600 MT/s, so the memory will run at 3600 MT/s."
            ),
            "{}",
            issue.message
        );
        // A four-module kit on its own, even one rated at the one-per-channel speed.
        let quad = memory_product("q", |s| s.modules = 4);
        let issue = only_issue(&check(&[&cpu, &board, &quad]), RuleId::MemorySpeed).clone();
        assert!(
            issue.message.contains("run at 3600 MT/s"),
            "{}",
            issue.message
        );
        // No CPU yet: the board alone cannot say how fast, only that it is slower.
        let issue = only_issue(&check(&[&board, &kit, &kit]), RuleId::MemorySpeed).clone();
        assert_eq!(issue.parts, [kit.id.clone(), board.id.clone()]);
        assert!(
            issue.message.contains("well below the rated speed"),
            "{}",
            issue.message
        );
        // Two modules: the usual limit.
        assert!(
            check(&[&cpu, &board, &memory_product("k", |s| s.speed_mts = 5600)])
                .issues
                .is_empty()
        );
    }

    #[test]
    fn speed_is_quiet_for_a_kit_that_does_not_fit() {
        let lga = |s: &mut techdepo_catalog::spec::CpuSpec| {
            s.socket = Socket::Lga1700;
            s.family = CpuFamily::Harrier;
            s.memory.push(MemorySupport {
                memory_type: MemoryType::Ddr4,
                max_speed_mts: 3200,
                max_speed_2dpc_mts: 2933,
            });
        };
        let cpu = cpu_product("c", lga);
        let ddr4_board = board_product("b", |s| {
            s.socket = Socket::Lga1700;
            s.chipset = techdepo_catalog::Chipset::B760;
            s.memory_type = MemoryType::Ddr4;
            s.max_memory_speed_mts = 3600;
            s.cpu_support[0].family = CpuFamily::Falcon;
            s.cpu_support[1].family = CpuFamily::Harrier;
        });
        let fast_ddr5 = memory_product("k", |s| s.speed_mts = 7200);
        let report = check(&[&cpu, &ddr4_board, &fast_ddr5, &fast_ddr5]);
        assert_eq!(fired(&report), [RuleId::MemoryType]);
        assert!(report.checked(RuleId::MemorySpeed) && report.checked(RuleId::MemoryMixedKits));
    }

    #[test]
    fn mixed_kits_note_slowest_settings() {
        let fast = memory_product("fast", |s| s.speed_mts = 6000);
        let slow = memory_product("slow", |s| {
            s.speed_mts = 5200;
            s.cas_latency = 40;
        });
        let issue = only_issue(&check(&[&fast, &slow]), RuleId::MemoryMixedKits).clone();
        assert!(
            issue.message.contains("5200 MT/s, CL40"),
            "{}",
            issue.message
        );
        let twins = only_issue(&check(&[&fast, &fast]), RuleId::MemoryMixedKits).clone();
        assert!(
            twins.message.contains("2× fast are separate kits"),
            "{}",
            twins.message
        );
    }
}
