//! Borrowed temperature cells keep readings and labels aligned within each row.

/// The dashboard reserves two equal-width cells per temperature row.
pub(super) const TEMP_COLUMNS: usize = 2;

/// Truncates to the visible matched prefix before splitting both borrowed slices.
pub(super) fn temperature_rows<'a, T, L>(
    temperatures: &'a [T],
    labels: &'a [L],
    shown: usize,
) -> impl Iterator<Item = (&'a [T], &'a [L])> {
    let count = shown.min(temperatures.len()).min(labels.len());
    temperatures[..count]
        .chunks(TEMP_COLUMNS)
        .zip(labels[..count].chunks(TEMP_COLUMNS))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Missing labels, visibility limits and an odd final cell preserve row order.
    #[test]
    fn rows_preserve_pairs_and_the_partial_final_row() {
        let temperatures = [10, 20, 30, 40, 50];
        let labels = ["a", "b", "c", "d", "e"];
        for shown in 0..=6 {
            let expected = temperatures
                .iter()
                .zip(&labels)
                .take(shown)
                .collect::<Vec<_>>();
            let actual = temperature_rows(&temperatures, &labels, shown)
                .flat_map(|(samples, labels)| samples.iter().zip(labels))
                .collect::<Vec<_>>();
            assert_eq!(actual, expected);
        }
        let rows = temperature_rows(&temperatures, &labels, usize::MAX).collect::<Vec<_>>();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2], (&temperatures[4..], &labels[4..]));
        assert!(std::ptr::eq(rows[0].0.as_ptr(), temperatures.as_ptr()));
        let short_labels = temperature_rows(&temperatures, &labels[..3], 5).collect::<Vec<_>>();
        assert_eq!(
            short_labels,
            [
                (&temperatures[..2], &labels[..2]),
                (&temperatures[2..3], &labels[2..3])
            ]
        );
        assert!(
            temperature_rows(&temperatures, &labels[..0], 5)
                .next()
                .is_none()
        );
        assert!(
            temperature_rows(&temperatures[..0], &labels, 5)
                .next()
                .is_none()
        );
    }
}
