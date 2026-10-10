use super::{Result, StoreError};

pub(super) fn unsigned(value: u64) -> Vec<u8> {
    value.to_be_bytes().to_vec()
}

pub(super) fn read_unsigned(bytes: &[u8]) -> Result<u64> {
    let array = <[u8; 8]>::try_from(bytes)
        .map_err(|error| StoreError::context("invalid unsigned History value", error))?;
    Ok(u64::from_be_bytes(array))
}

pub(super) fn signed(value: i128) -> Vec<u8> {
    let sortable = u128::from_be_bytes(value.to_be_bytes()) ^ (1_u128 << 127);
    sortable.to_be_bytes().to_vec()
}

#[cfg(test)]
pub(super) fn read_signed(bytes: &[u8]) -> Result<i128> {
    let array = <[u8; 16]>::try_from(bytes)
        .map_err(|error| StoreError::context("invalid signed History value", error))?;
    let restored = u128::from_be_bytes(array) ^ (1_u128 << 127);
    Ok(i128::from_be_bytes(restored.to_be_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsigned_order_crosses_signed_and_javascript_boundaries_exactly() -> Result<()> {
        let values = [
            0,
            1,
            (1_u64 << 53) - 1,
            1_u64 << 53,
            (1_u64 << 63) - 1,
            1_u64 << 63,
            u64::MAX,
        ];
        let mut previous = None;
        for value in values {
            let encoded = unsigned(value);
            assert_eq!(read_unsigned(&encoded)?, value);
            if let Some(previous) = previous {
                assert!(previous < encoded);
            }
            previous = Some(encoded);
        }
        Ok(())
    }

    #[test]
    fn signed_order_keeps_extreme_negative_change_and_zero() -> Result<()> {
        let largest_growth = (1 - i128::from(u64::MAX)) * 10_000;
        let values = [
            i128::MIN,
            largest_growth,
            i128::from(i64::MIN),
            -1,
            0,
            1,
            10_000,
            i128::MAX,
        ];
        let mut previous = None;
        for value in values {
            let encoded = signed(value);
            assert_eq!(read_signed(&encoded)?, value);
            if let Some(previous) = previous {
                assert!(previous < encoded);
            }
            previous = Some(encoded);
        }
        Ok(())
    }

    #[test]
    fn malformed_physical_numbers_are_rejected() {
        for bytes in [&[][..], &[0; 7][..], &[0; 9][..], &[0; 17][..]] {
            assert!(read_unsigned(bytes).is_err());
            assert!(read_signed(bytes).is_err());
        }
    }
}
