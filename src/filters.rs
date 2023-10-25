use byte_unit::Byte;
use std::collections::HashMap;

use tera::Value;

/// get a size in byte and returns a human readable version string
pub(crate) fn humanize_size(val: &Value, _args: &HashMap<String, Value>) -> tera::Result<Value> {
    if let Some(s) = val.as_i64() {
        match s.try_into() {
            Ok(x) => Ok(Byte::from_bytes(x)
                .get_appropriate_unit(false)
                .to_string()
                .into()),
            Err(err) => Err(tera::Error::msg(format!("invalid size: {err:?}"))),
        }
    } else {
        Err(tera::Error::msg(format!(
            "Invalid value, expected i64 but got {:?}",
            val
        )))
    }
}
