use serde_json::Value;

wit_bindgen::generate!({
    inline: r#"
        package helpcore:plugin;

        interface host {
            http-request: func(request-json: string) -> result<string, string>;
            data-read: func(path: string) -> result<string, string>;
            data-write: func(path: string, content: string) -> result<_, string>;
            config-read: func(key: string) -> result<string, string>;
        }

        world plugin {
            import host;
            export call: func(tool: string, input-json: string) -> result<string, string>;
        }
    "#,
    world: "plugin",
});

struct UnitConverter;

impl Guest for UnitConverter {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| format!("invalid input JSON: {e}"))?;

        match tool.as_str() {
            "convert" => convert(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(UnitConverter);

fn get_str<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

fn get_f64(input: &Value, key: &str) -> Result<f64, String> {
    input
        .get(key)
        .and_then(|v| v.as_f64())
        .ok_or_else(|| format!("{key} must be a number"))
}

fn convert(input: &Value) -> Result<String, String> {
    let value = get_f64(input, "value")?;
    let from = get_str(input, "from")?.to_lowercase();
    let to = get_str(input, "to")?.to_lowercase();

    // Temperature needs special handling (offset conversions)
    let is_temp = matches!(from.as_str(), "c" | "f" | "k")
        || matches!(to.as_str(), "c" | "f" | "k");

    let result = if is_temp {
        convert_temperature(value, &from, &to)?
    } else {
        let to_base = to_base_factor(&from)?;
        let from_base = to_base_factor(&to)?;
        value * (to_base / from_base)
    };

    let formatted = if result.is_nan() || result.is_infinite() {
        result.to_string()
    } else if result == 0.0 {
        "0".to_string()
    } else if result.abs() < 0.01 || result.abs() >= 1e15 {
        format!("{result:.10e}")
    } else {
        let s = format!("{result:.15}");
        let s = s.trim_end_matches('0');
        let s = s.trim_end_matches('.');
        s.to_string()
    };

    Ok(formatted)
}

fn to_base_factor(unit: &str) -> Result<f64, String> {
    match unit {
        // Length (base: meter)
        "mm" => Ok(0.001),
        "cm" => Ok(0.01),
        "m" => Ok(1.0),
        "km" => Ok(1000.0),
        "in" => Ok(0.0254),
        "ft" => Ok(0.3048),
        "yd" => Ok(0.9144),
        "mi" => Ok(1609.344),

        // Weight (base: gram)
        "mg" => Ok(0.001),
        "g" => Ok(1.0),
        "kg" => Ok(1000.0),
        "oz" => Ok(28.349523125),
        "lb" => Ok(453.59237),
        "st" => Ok(6350.29318),
        "ton" => Ok(1000000.0),
        "ton_us" => Ok(907184.74),

        // Volume (base: liter)
        "ml" => Ok(0.001),
        "l" => Ok(1.0),
        "gal" => Ok(3.785411784),
        "qt" => Ok(0.946352946),
        "pt" => Ok(0.473176473),
        "cup" => Ok(0.2365882365),
        "floz" => Ok(0.0295735295625),
        "tbsp" => Ok(0.01478676478125),
        "tsp" => Ok(0.00492892159375),

        // Speed (base: m/s)
        "mps" => Ok(1.0),
        "kmh" => Ok(1.0 / 3.6),
        "mph" => Ok(0.44704),
        "knot" => Ok(0.514444),

        // Data (base: byte)
        "b" => Ok(1.0),
        "kb" => Ok(1000.0),
        "mb" => Ok(1000000.0),
        "gb" => Ok(1000000000.0),
        "tb" => Ok(1000000000000.0),
        "kib" => Ok(1024.0),
        "mib" => Ok(1048576.0),
        "gib" => Ok(1073741824.0),
        "tib" => Ok(1099511627776.0),

        // Area (base: square meter)
        "m2" => Ok(1.0),
        "km2" => Ok(1000000.0),
        "ha" => Ok(10000.0),
        "ft2" => Ok(0.09290304),
        "acre" => Ok(4046.8564224),
        "mi2" => Ok(2589988.110336),

        // Time (base: second)
        "s" => Ok(1.0),
        "min" => Ok(60.0),
        "hr" => Ok(3600.0),
        "day" => Ok(86400.0),
        "week" => Ok(604800.0),
        "year" => Ok(31536000.0),

        // Pressure (base: pascal)
        "pa" => Ok(1.0),
        "kpa" => Ok(1000.0),
        "bar" => Ok(100000.0),
        "atm" => Ok(101325.0),
        "psi" => Ok(6894.757293168),
        "mmhg" => Ok(133.3223684211),

        // Energy (base: joule)
        "j" => Ok(1.0),
        "kj" => Ok(1000.0),
        "cal" => Ok(4.184),
        "kcal" => Ok(4184.0),
        "wh" => Ok(3600.0),
        "kwh" => Ok(3600000.0),

        // Angle (base: radian)
        "deg" => Ok(std::f64::consts::PI / 180.0),
        "rad" => Ok(1.0),
        "grad" => Ok(std::f64::consts::PI / 200.0),

        _ => Err(format!("unknown unit: {unit}")),
    }
}

fn convert_temperature(value: f64, from: &str, to: &str) -> Result<f64, String> {
    // Convert to Celsius first
    let celsius = match from {
        "c" => value,
        "f" => (value - 32.0) * 5.0 / 9.0,
        "k" => value - 273.15,
        _ => return Err(format!("unknown temperature unit: {from}")),
    };
    let result = match to {
        "c" => celsius,
        "f" => celsius * 9.0 / 5.0 + 32.0,
        "k" => celsius + 273.15,
        _ => return Err(format!("unknown temperature unit: {to}")),
    };
    Ok(result)
}
