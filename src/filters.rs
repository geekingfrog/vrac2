use byte_unit::Byte;

use tera::{State, TeraResult, Value};

/// get a size in byte and returns a human readable version string
pub(crate) fn humanize_size(
    val: &Value,
    _kwargs: tera::Kwargs,
    _state: &State,
) -> TeraResult<Value> {
    if let Some(s) = val.as_i64() {
        match s.try_into() {
            Ok(x) => Ok(Byte::from_bytes(x)
                .get_appropriate_unit(false)
                .to_string()
                .into()),
            Err(err) => Err(tera::Error::message(format!("invalid size: {err:?}"))),
        }
    } else {
        Err(tera::Error::message(format!(
            "Invalid value, expected i64 but got {:?}",
            val
        )))
    }
}
