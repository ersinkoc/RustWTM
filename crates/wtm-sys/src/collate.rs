//! Text order as people expect it in a list: "file2" before "file10",
//! "Édge" next to "edge", capitals mixed in with small letters.
//!
//! Windows' own linguistic comparison does the work, with the invariant
//! locale so the order is the same on every PC (a Turkish locale would
//! otherwise sort "I" and "i" apart from everywhere else).

use std::cell::RefCell;
use std::cmp::Ordering;

use windows::Win32::Globalization::{
    CSTR_EQUAL, CSTR_GREATER_THAN, CSTR_LESS_THAN, CompareStringEx, LINGUISTIC_IGNORECASE,
    LINGUISTIC_IGNOREDIACRITIC, LOCALE_NAME_INVARIANT, NORM_IGNOREKANATYPE, NORM_IGNOREWIDTH,
    SORT_DIGITSASNUMBERS,
};

thread_local! {
    /// Two reused UTF-16 buffers: sorting compares the same few hundred
    /// strings thousands of times, so nothing is allocated per comparison.
    static BUFFERS: RefCell<(Vec<u16>, Vec<u16>)> = const { RefCell::new((Vec::new(), Vec::new())) };
}

/// Compares `a` and `b` ignoring case, accents and full/half width, with
/// runs of digits compared by their value. Strings Windows calls equal (or
/// that it cannot compare) fall back to a plain lower-case comparison, so
/// the order stays total and repeatable.
pub fn natural(a: &str, b: &str) -> Ordering {
    let linguistic = BUFFERS
        .try_with(|cell| {
            let Ok(mut buffers) = cell.try_borrow_mut() else {
                return None;
            };
            let (left, right) = &mut *buffers;
            left.clear();
            left.extend(a.encode_utf16());
            right.clear();
            right.extend(b.encode_utf16());
            compare(left, right)
        })
        .ok()
        .flatten();
    match linguistic {
        Some(Ordering::Equal) | None => lower(a, b),
        Some(order) => order,
    }
}

fn compare(a: &[u16], b: &[u16]) -> Option<Ordering> {
    // Lengths beyond i32 are rejected by the binding; such strings are never
    // shown in a list, and the caller falls back to a plain comparison.
    if i32::try_from(a.len()).is_err() || i32::try_from(b.len()).is_err() {
        return None;
    }
    let flags = SORT_DIGITSASNUMBERS
        | LINGUISTIC_IGNORECASE
        | LINGUISTIC_IGNOREDIACRITIC
        | NORM_IGNOREKANATYPE
        | NORM_IGNOREWIDTH;
    // SAFETY: both slices are valid for their lengths for the whole call; no
    // version information, reserved pointer or handle is passed.
    let result = unsafe { CompareStringEx(LOCALE_NAME_INVARIANT, flags, a, b, None, None, None) };
    match result {
        CSTR_LESS_THAN => Some(Ordering::Less),
        CSTR_EQUAL => Some(Ordering::Equal),
        CSTR_GREATER_THAN => Some(Ordering::Greater),
        _ => None,
    }
}

fn lower(a: &str, b: &str) -> Ordering {
    a.chars()
        .flat_map(char::to_lowercase)
        .cmp(b.chars().flat_map(char::to_lowercase))
        .then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(mut v: Vec<&str>) -> Vec<&str> {
        v.sort_by(|a, b| natural(a, b));
        v
    }

    #[test]
    fn digit_runs_compare_by_value() {
        assert_eq!(
            sorted(vec!["file10", "file2", "file1", "File3"]),
            ["file1", "file2", "File3", "file10"]
        );
        assert_eq!(natural("svchost (12)", "svchost (9)"), Ordering::Greater);
    }

    #[test]
    fn case_accents_and_width_do_not_split_names() {
        assert_eq!(
            sorted(vec!["zed", "Édge", "edit", "Apple"]),
            ["Apple", "Édge", "edit", "zed"]
        );
        // Full-width letters sort with their ASCII counterparts.
        assert_eq!(sorted(vec!["c", "ｂ", "a"]), ["a", "ｂ", "c"]);
    }

    #[test]
    fn equal_looking_names_still_have_a_fixed_order() {
        assert_ne!(natural("Edge", "edge"), Ordering::Equal);
        assert_eq!(natural("Edge", "edge"), natural("Edge", "edge"));
        assert_eq!(natural("Edge", "edge"), natural("edge", "Edge").reverse());
        assert_eq!(natural("", ""), Ordering::Equal);
        assert_eq!(natural("", "a"), Ordering::Less);
    }

    #[test]
    fn the_order_does_not_depend_on_turkish_casing() {
        // With a Turkish locale "I" would not match "i"; the invariant locale keeps them together.
        assert_eq!(sorted(vec!["ivy", "Ice", "hat"]), ["hat", "Ice", "ivy"]);
    }
}
