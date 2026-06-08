use serde::{Deserialize, Serialize};
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

use helpcore::plugin::host;

struct NutritionFacts;

impl Guest for NutritionFacts {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| format!("invalid input JSON: {e}"))?;
        match tool.as_str() {
            "food_search" => food_search(&input),
            "barcode_lookup" => barcode_lookup(&input),
            "compare_foods" => compare_foods(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(NutritionFacts);

fn get_str<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

#[derive(Serialize)]
struct HttpReq<'a> { method: &'a str, url: String, headers: serde_json::Map<String, Value>, #[serde(skip_serializing_if = "Option::is_none")] body: Option<String> }
#[derive(Deserialize)]
struct HttpResp { status: u16, body: String }

fn http_get(url: &str) -> Result<String, String> {
    let mut headers = serde_json::Map::new();
    headers.insert("User-Agent".to_string(), Value::String("helpcore-nutrition/1.0".to_string()));
    let req = HttpReq { method: "GET", url: url.to_string(), headers, body: None };
    let req_json = serde_json::to_string(&req).map_err(|e| format!("serialize: {e}"))?;
    let resp_json = host::http_request(&req_json)?;
    let resp: HttpResp = serde_json::from_str(&resp_json).map_err(|e| format!("parse HTTP: {e}"))?;
    if resp.status >= 400 { return Err(format!("HTTP {}: {}", resp.status, resp.body)); }
    Ok(resp.body)
}

fn food_search(input: &Value) -> Result<String, String> {
    let food = get_str(input, "food")?;
    let url = format!("https://world.openfoodfacts.org/cgi/search.pl?search_terms={}&search_simple=1&json=1&page_size=5", food.replace(' ', "+"));
    let body = http_get(&url)?;
    let data: Value = serde_json::from_str(&body).map_err(|e| format!("parse: {e}"))?;
    let products = data["products"].as_array().ok_or_else(|| format!("No results for '{}'.", food))?;

    if products.is_empty() {
        return Err(format!("No nutrition data found for '{}'. Try a more specific food name.", food));
    }

    let mut out = format!("Nutrition info for '{}':\n\n", food);
    for (i, p) in products.iter().take(3).enumerate() {
        out.push_str(&format_product(p));
        if i < 2 { out.push_str("\n---\n\n"); }
    }
    Ok(out)
}

fn barcode_lookup(input: &Value) -> Result<String, String> {
    let barcode = get_str(input, "barcode")?;
    let url = format!("https://world.openfoodfacts.org/api/v0/product/{}.json", barcode);
    let body = http_get(&url)?;
    let data: Value = serde_json::from_str(&body).map_err(|e| format!("parse: {e}"))?;

    if data["status"].as_u64() == Some(0) {
        return Err(format!("Barcode '{}' not found in Open Food Facts database.", barcode));
    }

    let product = &data["product"];
    Ok(format_product(product))
}

fn compare_foods(input: &Value) -> Result<String, String> {
    let food1 = get_str(input, "food1")?;
    let food2 = get_str(input, "food2")?;

    let items = [food1, food2];
    let mut results = Vec::new();

    for &food in &items {
        let url = format!("https://world.openfoodfacts.org/cgi/search.pl?search_terms={}&search_simple=1&json=1&page_size=1", food.replace(' ', "+"));
        let body = http_get(&url)?;
        let data: Value = serde_json::from_str(&body).map_err(|e| format!("parse: {e}"))?;
        let products = data["products"].as_array();
        if let Some(prods) = products {
            if let Some(p) = prods.first() {
                results.push(extract_nutriments(p));
                continue;
            }
        }
        return Err(format!("Could not find '{}'.", food));
    }

    let (n1, name1) = &results[0];
    let (n2, name2) = &results[1];

    let mut out = format!("Comparison per 100g:\n\n");
    out.push_str(&format!("{:<25} {:<25} {:<25}\n", "", name1, name2));
    out.push_str(&format!("{:-<75}\n", ""));

    for key in &["energy_kcal", "proteins", "carbohydrates", "sugars", "fat", "saturated_fat", "fiber", "salt", "sodium"] {
        let v1 = n1.get(*key).and_then(|v| v.as_str()).unwrap_or("—");
        let v2 = n2.get(*key).and_then(|v| v.as_str()).unwrap_or("—");
        let label = match *key {
            "energy_kcal" => "Calories",
            "proteins" => "Protein",
            "carbohydrates" => "Carbs",
            "sugars" => "Sugars",
            "fat" => "Fat",
            "saturated_fat" => "Sat Fat",
            "fiber" => "Fiber",
            "salt" => "Salt",
            "sodium" => "Sodium",
            _ => *key,
        };
        let unit = if *key == "energy_kcal" { "kcal" } else { "g" };
        out.push_str(&format!("{:<25} {:<25} {:<25}\n", label, format!("{v1} {unit}"), format!("{v2} {unit}")));
    }

    Ok(out)
}

fn extract_nutriments(product: &Value) -> (serde_json::Map<String, Value>, String) {
    let name = product.get("product_name")
        .and_then(Value::as_str)
        .or(product.get("generic_name").and_then(Value::as_str))
        .unwrap_or("Unknown");

    let nutriments = product.get("nutriments")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    (nutriments, name.to_string())
}

fn format_product(product: &Value) -> String {
    let name = product.get("product_name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or(product.get("generic_name").and_then(Value::as_str))
        .unwrap_or("Unknown product");

    let brand = product.get("brands").and_then(Value::as_str).unwrap_or("");
    let qty = product.get("quantity").and_then(Value::as_str).unwrap_or("");
    let image = product.get("image_url").and_then(Value::as_str).unwrap_or("");

    let nutriments = product.get("nutriments").and_then(Value::as_object);

    let mut out = format!("**{name}**\n");
    if !brand.is_empty() { out.push_str(&format!("Brand: {brand}\n")); }
    if !qty.is_empty() { out.push_str(&format!("Size: {qty}\n")); }
    out.push_str("\nPer 100g:\n");

    if let Some(nut) = nutriments {
        let items = [
            ("energy-kcal_100g", "Calories", "kcal"),
            ("proteins_100g", "Protein", "g"),
            ("carbohydrates_100g", "Carbs", "g"),
            ("sugars_100g", "Sugars", "g"),
            ("fat_100g", "Fat", "g"),
            ("saturated-fat_100g", "Saturated Fat", "g"),
            ("fiber_100g", "Fiber", "g"),
            ("salt_100g", "Salt", "g"),
        ];
        for (key, label, unit) in &items {
            if let Some(v) = nut.get(*key).and_then(Value::as_f64) {
                out.push_str(&format!("  {label}: {v} {unit}\n"));
            }
        }

        // Allergens
        if let Some(allergens) = product.get("allergens_tags").and_then(Value::as_array) {
            let tags: Vec<&str> = allergens.iter().filter_map(Value::as_str)
                .map(|s| s.trim_start_matches("en:"))
                .collect();
            if !tags.is_empty() {
                out.push_str(&format!("\nAllergens: {}\n", tags.join(", ")));
            }
        }

        // Ingredients
        if let Some(ingredients) = product.get("ingredients_text").and_then(Value::as_str) {
            if ingredients.len() <= 200 {
                out.push_str(&format!("\nIngredients: {ingredients}\n"));
            } else {
                out.push_str(&format!("\nIngredients: {}...\n", &ingredients[..197]));
            }
        }
    }

    if !image.is_empty() {
        out.push_str(&format!("\nImage: {image}"));
    }

    out
}
