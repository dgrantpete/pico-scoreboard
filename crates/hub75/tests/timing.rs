//! Timing buffer, refresh-rate estimator, and target search vs the real
//! driver.py methods. Float goldens compare bit-exactly: both sides are
//! IEEE f64 evaluating the same expression sequence.

#[path = "goldens/mod.rs"]
mod goldens;

use hub75::timing;

#[test]
fn timing_words_match_python() {
    for case in &goldens::TIMING_CASES {
        let words = timing::timing_words(
            case.base_cycles as u64,
            case.brightness,
            case.blanking_ns,
            case.system_hz,
        );
        assert_eq!(
            words, case.words,
            "timing mismatch: base={} brightness={} blanking={} sys={}",
            case.base_cycles, case.brightness, case.blanking_ns, case.system_hz
        );
    }
}

#[test]
fn refresh_estimates_match_python() {
    for case in &goldens::ESTIMATE_CASES {
        let rate = timing::estimate_refresh_rate(
            case.base_cycles as u64,
            case.brightness,
            case.blanking_ns,
            case.system_hz,
            case.data_hz,
        );
        assert_eq!(
            rate.to_bits(),
            case.rate_bits,
            "estimate mismatch: base={} brightness={} blanking={} sys={} data={}: got {rate}, expected {}",
            case.base_cycles,
            case.brightness,
            case.blanking_ns,
            case.system_hz,
            case.data_hz,
            f64::from_bits(case.rate_bits)
        );
    }
}

#[test]
fn target_search_matches_python() {
    for case in &goldens::TARGET_CASES {
        let (base_cycles, rate) = timing::base_cycles_for_target(
            case.target_hz,
            case.brightness,
            case.blanking_ns,
            case.system_hz,
            case.data_hz,
        );
        assert_eq!(
            base_cycles, case.base_cycles as u64,
            "base_cycles mismatch for target {} Hz",
            case.target_hz
        );
        assert_eq!(
            rate.to_bits(),
            case.rate_bits,
            "rate mismatch for target {} Hz: got {rate}, expected {}",
            case.target_hz,
            f64::from_bits(case.rate_bits)
        );
        let words = timing::timing_words(
            base_cycles,
            case.brightness,
            case.blanking_ns,
            case.system_hz,
        );
        assert_eq!(words, case.words, "timing mismatch for target {} Hz", case.target_hz);
    }
}

#[test]
fn timing_word_structure() {
    // Plane i's on-window is base << i (the BCM weighting) at brightness 1.0,
    // where the entire window is spent lit and off is pure blanking.
    let words = timing::timing_words(3, 1.0, 1000, 150_000_000);
    let blanking_cycles = 150; // 1000 ns at 150 MHz
    for plane in 0..8 {
        assert_eq!(words[plane * 2], blanking_cycles, "off word, plane {plane}");
        assert_eq!(words[plane * 2 + 1], 3 << plane, "on word, plane {plane}");
    }

    // At brightness 0 nothing is ever lit and the off delay absorbs the
    // whole window, halved for its two runs per bitframe.
    let words = timing::timing_words(4, 0.0, 0, 150_000_000);
    for plane in 0..8 {
        assert_eq!(words[plane * 2], (4u32 << plane) / 2);
        assert_eq!(words[plane * 2 + 1], 0);
    }
}
