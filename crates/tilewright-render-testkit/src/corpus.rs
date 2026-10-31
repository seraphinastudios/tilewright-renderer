// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

/// Maximum absolute difference in one RGBA8 channel for a named corpus case.
///
/// Software Vulkan measurements require two steps for these two fixtures;
/// every other case, including unknown names, retains the one-step contract.
/// This allowance never applies to semantic, depth or coverage planes.
/// See `corpus/README.md` for the measured color contract and provenance.
#[must_use]
pub fn corpus_maximum_rgba_channel_difference(name: &str) -> u8 {
    match name {
        "structural-fog-ui" | "shared-ui-dpi-125" => 2,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::corpus_maximum_rgba_channel_difference;

    #[test]
    fn measured_exceptions_do_not_relax_other_corpus_cases() {
        // An unexpected two-step error must still fail every other fixture,
        // including the same shared UI at 1.5 DPI and misspelled/new cases.
        let difference = 24_u8.abs_diff(22);
        for name in [
            "overlap-square",
            "unaligned-wide",
            "high-density-1024",
            "world-iso-terrain",
            "world-top-down",
            "atlas-layered-characters",
            "shared-ui-dpi-150",
            "shared-ui-dpi-126",
            "new-case",
        ] {
            let allowance = corpus_maximum_rgba_channel_difference(name);
            assert_eq!(allowance, 1, "{name}");
            assert!(
                difference > allowance,
                "{name} accepted an unexpected error"
            );
        }
        for name in ["structural-fog-ui", "shared-ui-dpi-125"] {
            let allowance = corpus_maximum_rgba_channel_difference(name);
            assert_eq!(allowance, 2, "{name}");
            assert!(difference <= allowance, "{name}");
            assert!(
                25_u8.abs_diff(22) > allowance,
                "{name} accepted three steps"
            );
        }
    }
}
