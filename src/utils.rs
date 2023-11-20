use serde_bencode::value::Value as BcValue;
use serde_json as js;

pub(crate) fn bencode_strings(val: &BcValue) -> js::Value {
    match val {
        BcValue::Int(n) => js::Value::Number((*n).into()),
        BcValue::Bytes(bytes) => js::Value::String(String::from_utf8_lossy(bytes).to_string()),
        BcValue::List(vals) => js::Value::Array(vals.iter().map(bencode_strings).collect()),
        BcValue::Dict(vals) => js::Value::Object(
            vals.iter()
                .map(|(k, v)| (String::from_utf8_lossy(k).to_string(), bencode_strings(v)))
                .collect::<js::Map<String, js::Value>>(),
        ),
    }
}
