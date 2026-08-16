use crate::contracts::{AppErrorCode, CommandError, SafeParameterValue};

pub fn validate_local_date(value: &str) -> Result<(), CommandError> {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes
            .iter()
            .enumerate()
            .any(|(index, byte)| !matches!(index, 4 | 7) && !byte.is_ascii_digit())
    {
        return Err(invalid_input());
    }

    let year = value[0..4].parse::<u32>().map_err(|_| invalid_input())?;
    let month = value[5..7].parse::<u32>().map_err(|_| invalid_input())?;
    let day = value[8..10].parse::<u32>().map_err(|_| invalid_input())?;
    if !(1..=9_999).contains(&year) || !(1..=12).contains(&month) {
        return Err(invalid_input());
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days_in_month = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if !(1..=days_in_month).contains(&day) {
        return Err(invalid_input());
    }
    Ok(())
}

pub fn note_excerpt(markdown: &str) -> String {
    markdown
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(160)
        .collect()
}

fn invalid_input() -> CommandError {
    CommandError::with_detail(
        AppErrorCode::InvalidInput,
        "errors.invalidInput",
        "field",
        SafeParameterValue::String("noteDate".into()),
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::{note_excerpt, validate_local_date};
    use crate::contracts::{AppErrorCode, SafeParameterValue};
    use std::collections::BTreeMap;

    #[test]
    fn validates_real_gregorian_dates_including_leap_days() {
        assert!(validate_local_date("2026-02-30").is_err());
        assert!(validate_local_date("2028-02-29").is_ok());
        assert!(validate_local_date("0000-12-31").is_err());
        assert!(validate_local_date("9999-12-31").is_ok());
    }

    #[test]
    fn invalid_dates_identify_only_the_safe_note_date_field() {
        for value in ["not-a-date/private", "2026-02-30"] {
            let error = validate_local_date(value).unwrap_err();
            assert_eq!(error.code, AppErrorCode::InvalidInput);
            assert_eq!(error.message_key, "errors.invalidInput");
            assert_eq!(
                error.details,
                BTreeMap::from([(
                    "field".into(),
                    SafeParameterValue::String("noteDate".into()),
                )])
            );
            assert!(!format!("{:?}", error.details).contains(value));
        }
    }

    #[test]
    fn excerpt_collapses_whitespace_before_taking_unicode_scalars() {
        let markdown = format!("  alpha\n\t beta   {}tail", "界".repeat(158));
        let excerpt = note_excerpt(&markdown);
        assert_eq!(excerpt.chars().count(), 160);
        assert!(excerpt.starts_with("alpha beta 界"));
        assert!(!excerpt.contains('\n'));
        assert!(!excerpt.ends_with("tail"));
    }
}
