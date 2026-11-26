//! Loudness-Matched Blind ABX Comparison Lab math & trial state.
//!
//! Uses exact upper-tail binomial probability `P(X >= k | n, p = 0.5)` and
//! EBU R128 Integrated LUFS gain trimming so louder mastering never biases blind listening.

use rand::Rng;

/// Exact two-choice blind ABX upper-tail p-value: probability of scoring at least `correct`
/// out of `trials` purely by guessing (`p = 0.5`).
pub fn binomial_p_value(correct: usize, trials: usize) -> f64 {
    if trials == 0 {
        return 1.0;
    }
    if correct == 0 {
        return 1.0;
    }
    if correct > trials {
        return 0.0;
    }
    // Compute sum_{k=correct}^{trials} C(trials, k) * 0.5^trials using log-space / iterative ratio
    let n = trials as f64;
    let inv_pow = 0.5_f64.powi(trials as i32);
    // Start at C(n, 0) = 1.0 and step up to k = n cleanly (numerically exact for typical ABX n <= 100)
    let mut c = 1.0_f64;
    let mut sum = 0.0_f64;
    for k in 0..=trials {
        if k >= correct {
            sum += c * inv_pow;
        }
        if k < trials {
            c *= (n - k as f64) / ((k + 1) as f64);
        }
    }
    sum.clamp(0.0, 1.0)
}

/// Computes exact loudness-matching attenuation (dB <= 0.0) for Track A and Track B
/// so both play at the quieter track's Integrated LUFS (zero clipping!).
pub fn compute_loudness_match_trims(
    lufs_a: Option<f32>,
    lufs_b: Option<f32>,
) -> (f32, f32) {
    match (lufs_a, lufs_b) {
        (Some(a), Some(b)) if a.is_finite() && b.is_finite() => {
            let target = a.min(b);
            let trim_a = (target - a).clamp(-24.0, 0.0);
            let trim_b = (target - b).clamp(-24.0, 0.0);
            (trim_a, trim_b)
        }
        _ => (0.0, 0.0),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbxSlot {
    A,
    B,
    X,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct AbxTrialRecord {
    pub trial_num: usize,
    pub x_was_b: bool,
    pub guessed_b: bool,
    pub correct: bool,
}

#[derive(Debug, Clone)]
pub struct AbxLabState {
    pub open: bool,
    pub track_a_id: Option<u64>,
    pub track_b_id: Option<u64>,
    pub loudness_matched: bool,
    pub active_slot: Option<AbxSlot>,
    pub current_x_is_b: bool,
    pub revealed: bool,
    pub history: Vec<AbxTrialRecord>,
}

impl Default for AbxLabState {
    fn default() -> Self {
        Self {
            open: false,
            track_a_id: None,
            track_b_id: None,
            loudness_matched: true,
            active_slot: None,
            current_x_is_b: rand::thread_rng().r#gen::<bool>(),
            revealed: false,
            history: Vec::new(),
        }
    }
}

impl AbxLabState {
    pub fn reset_trials(&mut self) {
        self.history.clear();
        self.revealed = false;
        self.current_x_is_b = rand::thread_rng().r#gen::<bool>();
    }

    pub fn record_guess(&mut self, guessed_b: bool) -> bool {
        let correct = guessed_b == self.current_x_is_b;
        let trial_num = self.history.len() + 1;
        self.history.push(AbxTrialRecord {
            trial_num,
            x_was_b: self.current_x_is_b,
            guessed_b,
            correct,
        });
        // Immediately randomize X for next blind trial
        self.current_x_is_b = rand::thread_rng().r#gen::<bool>();
        correct
    }

    pub fn correct_count(&self) -> usize {
        self.history.iter().filter(|r| r.correct).count()
    }

    pub fn p_value(&self) -> f64 {
        binomial_p_value(self.correct_count(), self.history.len())
    }

    pub fn resolved_track_id(&self, slot: AbxSlot) -> Option<u64> {
        match slot {
            AbxSlot::A => self.track_a_id,
            AbxSlot::B => self.track_b_id,
            AbxSlot::X => {
                if self.current_x_is_b {
                    self.track_b_id
                } else {
                    self.track_a_id
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_binomial_p_values_match_statistical_tables() {
        // 10/10 = 1/1024 = 0.0009765625
        let p10 = binomial_p_value(10, 10);
        assert!((p10 - (1.0 / 1024.0)).abs() < 1e-9, "got {p10}");
        // 9/10 = 11/1024 = 0.0107421875
        let p9 = binomial_p_value(9, 10);
        assert!((p9 - (11.0 / 1024.0)).abs() < 1e-9, "got {p9}");
        // 5/10 = 638/1024 = 0.623046875
        let p5 = binomial_p_value(5, 10);
        assert!((p5 - (638.0 / 1024.0)).abs() < 1e-9, "got {p5}");
    }

    #[test]
    fn loudness_match_attenuates_only_louder_master() {
        // Track A is -10.0 LUFS (loud modern), Track B is -16.4 LUFS (dynamic original)
        let (ta, tb) = compute_loudness_match_trims(Some(-10.0), Some(-16.4));
        assert!((ta - (-6.4)).abs() < 1e-4, "expected -6.4 dB trim on A, got {ta}");
        assert!(tb.abs() < 1e-4, "expected 0.0 dB trim on B, got {tb}");
    }
}
