use std::collections::HashMap;

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

struct MealFinder;

impl Guest for MealFinder {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| format!("invalid input JSON: {e}"))?;
        match tool.as_str() {
            "meal_search" => meal_search(&input),
            "meal_by_ingredient" => meal_by_ingredient(&input),
            "meal_detail" => meal_detail(&input),
            "meal_plan" => meal_plan(&input),
            "shopping_list" => shopping_list(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(MealFinder);

fn get_str<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

#[derive(Serialize)]
struct HttpReq<'a> { method: &'a str, url: String, headers: serde_json::Map<String, Value>, #[serde(skip_serializing_if = "Option::is_none")] body: Option<String> }
#[derive(Deserialize)]
struct HttpResp { status: u16, body: String }

fn http_get(url: &str) -> Result<String, String> {
    let req = HttpReq { method: "GET", url: url.to_string(), headers: serde_json::Map::new(), body: None };
    let req_json = serde_json::to_string(&req).map_err(|e| format!("serialize: {e}"))?;
    let resp_json = host::http_request(&req_json)?;
    let resp: HttpResp = serde_json::from_str(&resp_json).map_err(|e| format!("parse HTTP: {e}"))?;
    if resp.status >= 400 { return Err(format!("HTTP {}: {}", resp.status, resp.body)); }
    Ok(resp.body)
}

fn meal_search(input: &Value) -> Result<String, String> {
    let query = get_str(input, "query")?;
    if query.eq_ignore_ascii_case("random") {
        let body = http_get("https://www.themealdb.com/api/json/v1/1/random.php")?;
        let data: Value = serde_json::from_str(&body).map_err(|e| format!("parse: {e}"))?;
        if let Some(meals) = data["meals"].as_array() {
            if let Some(meal) = meals.first() {
                return Ok(format_meal(meal));
            }
        }
        return Err("No random meal found.".to_string());
    }

    let url = format!("https://www.themealdb.com/api/json/v1/1/search.php?s={}", query.replace(' ', "%20"));
    let body = http_get(&url)?;
    let data: Value = serde_json::from_str(&body).map_err(|e| format!("parse: {e}"))?;
    let meals = data["meals"].as_array().ok_or_else(|| format!("No meals found for '{}'. Try a different search term.", query))?;

    if meals.len() == 1 {
        return Ok(format_meal(&meals[0]));
    }

    let mut out = format!("Found {} meals for '{}':\n\n", meals.len(), query);
    for meal in meals.iter().take(10) {
        let id = meal["idMeal"].as_str().unwrap_or("");
        let name = meal["strMeal"].as_str().unwrap_or("");
        let cat = meal["strCategory"].as_str().unwrap_or("");
        let area = meal["strArea"].as_str().unwrap_or("");
        let thumb = meal["strMealThumb"].as_str().unwrap_or("");
        out.push_str(&format!("**{name}** [{id}]\n  Category: {cat} | Cuisine: {area}\n  Image: {thumb}\n\n"));
    }
    if meals.len() > 10 {
        out.push_str(&format!("... and {} more. Refine your search for fewer results.\n", meals.len() - 10));
    }
    Ok(out)
}

fn meal_by_ingredient(input: &Value) -> Result<String, String> {
    let ingredient = get_str(input, "ingredient")?;
    let url = format!("https://www.themealdb.com/api/json/v1/1/filter.php?i={}", ingredient.replace(' ', "_"));
    let body = http_get(&url)?;
    let data: Value = serde_json::from_str(&body).map_err(|e| format!("parse: {e}"))?;
    let meals = data["meals"].as_array().ok_or_else(|| format!("No meals found with ingredient '{}'.", ingredient))?;

    let mut out = format!("Meals with {}:\n\n", ingredient);
    for meal in meals.iter().take(15) {
        let id = meal["idMeal"].as_str().unwrap_or("");
        let name = meal["strMeal"].as_str().unwrap_or("");
        let thumb = meal["strMealThumb"].as_str().unwrap_or("");
        out.push_str(&format!("**{name}** [{id}]\n  {thumb}\n\n"));
    }
    Ok(out)
}

fn meal_detail(input: &Value) -> Result<String, String> {
    let meal_id = get_str(input, "meal_id")?;
    let url = format!("https://www.themealdb.com/api/json/v1/1/lookup.php?i={meal_id}");
    let body = http_get(&url)?;
    let data: Value = serde_json::from_str(&body).map_err(|e| format!("parse: {e}"))?;
    let meals = data["meals"].as_array().and_then(|a| a.first())
        .ok_or_else(|| format!("Meal ID '{}' not found.", meal_id))?;

    Ok(format_meal_detail(meals))
}

fn format_meal(meal: &Value) -> String {
    let id = meal["idMeal"].as_str().unwrap_or("");
    let name = meal["strMeal"].as_str().unwrap_or("");
    let cat = meal["strCategory"].as_str().unwrap_or("");
    let area = meal["strArea"].as_str().unwrap_or("");
    let thumb = meal["strMealThumb"].as_str().unwrap_or("");
    let tags = meal["strTags"].as_str().filter(|s| !s.is_empty()).map(|s| format!("Tags: {s}\n")).unwrap_or_default();

    // Count ingredients
    let mut count = 0;
    for i in 1..=20 {
        let key = format!("strIngredient{i}");
        if meal.get(&key).and_then(Value::as_str).map_or(true, |s| s.is_empty()) { break; }
        count += 1;
    }

    format!(
        "**{name}** [{id}]\n\
         Category: {cat} | Cuisine: {area}\n{tags}\
         Ingredients: {count}\n\
         Image: {thumb}\n\n\
         Use meal_detail with ID {id} for the full recipe.",
    )
}

fn format_meal_detail(meal: &Value) -> String {
    let name = meal["strMeal"].as_str().unwrap_or("");
    let cat = meal["strCategory"].as_str().unwrap_or("");
    let area = meal["strArea"].as_str().unwrap_or("");
    let thumb = meal["strMealThumb"].as_str().unwrap_or("");
    let instructions = meal["strInstructions"].as_str().unwrap_or("");
    let video = meal["strYoutube"].as_str().filter(|s| !s.is_empty()).map(|v| format!("Video: {v}\n")).unwrap_or_default();

    let mut ingredients = Vec::new();
    for i in 1..=20 {
        let ing_key = format!("strIngredient{i}");
        let meas_key = format!("strMeasure{i}");
        if let Some(ing) = meal.get(&ing_key).and_then(Value::as_str) {
            if ing.is_empty() { break; }
            let meas = meal.get(&meas_key).and_then(Value::as_str).unwrap_or("");
            ingredients.push(format!("  - {} {}", meas.trim(), ing));
        }
    }

    format!(
        "**{name}**\n\
         Category: {cat} | Cuisine: {area}\n\
         {video}\n\
         Ingredients:\n{ingredients}\n\n\
         Instructions:\n{instructions}\n\n\
         Image: {thumb}",
        ingredients = ingredients.join("\n"),
        instructions = instructions
    )
}

fn meal_plan(input: &Value) -> Result<String, String> {
    let days = input.get("days").and_then(Value::as_i64).unwrap_or(7).max(1).min(14) as usize;
    let meals_per_day = input.get("meals_per_day").and_then(Value::as_i64).unwrap_or(3).max(1).min(5) as usize;

    let slot_names: Vec<String> = match meals_per_day {
        3 => vec!["Breakfast".into(), "Lunch".into(), "Dinner".into()],
        2 => vec!["Lunch".into(), "Dinner".into()],
        _ => (1..=meals_per_day).map(|i| format!("Meal {}", i)).collect(),
    };

    let day_names = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

    let mut all_ids: Vec<String> = Vec::new();
    let mut out = String::new();

    for day in 0..days {
        let day_name = day_names[day % 7];
        out.push_str(&format!("Day {} ({}):\n", day + 1, day_name));
        for slot in 0..meals_per_day {
            let body = http_get("https://www.themealdb.com/api/json/v1/1/random.php")?;
            let data: Value = serde_json::from_str(&body).map_err(|e| format!("parse: {e}"))?;
            if let Some(meals) = data["meals"].as_array() {
                if let Some(meal) = meals.first() {
                    let id = meal["idMeal"].as_str().unwrap_or("");
                    let name = meal["strMeal"].as_str().unwrap_or("");
                    all_ids.push(id.to_string());
                    out.push_str(&format!("  {}: {name} [{id}]\n", slot_names[slot]));
                }
            }
        }
        out.push('\n');
    }

    out.push_str(&format!("---\nTotal meals: {}\n", all_ids.len()));
    if !all_ids.is_empty() {
        out.push_str(&format!("Meal IDs: {}\n", all_ids.join(", ")));
    }

    Ok(out)
}

fn shopping_list(input: &Value) -> Result<String, String> {
    let meal_ids = get_str(input, "meal_ids")?;
    let ids: Vec<&str> = meal_ids.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    if ids.is_empty() {
        return Err("meal_ids must contain at least one ID".to_string());
    }

    let mut ingredient_map: HashMap<String, (String, Vec<String>)> = HashMap::new();

    for id in &ids {
        let url = format!("https://www.themealdb.com/api/json/v1/1/lookup.php?i={id}");
        let body = http_get(&url)?;
        let data: Value = serde_json::from_str(&body).map_err(|e| format!("parse: {e}"))?;
        let meals = data["meals"].as_array()
            .ok_or_else(|| format!("Meal ID '{}' not found.", id))?;
        let meal = meals.first()
            .ok_or_else(|| format!("Meal ID '{}' not found.", id))?;

        for i in 1..=20 {
            let ing_key = format!("strIngredient{i}");
            let meas_key = format!("strMeasure{i}");
            if let Some(ing) = meal.get(&ing_key).and_then(Value::as_str) {
                let ing = ing.trim();
                if ing.is_empty() { continue; }
                let meas = meal.get(&meas_key)
                    .and_then(Value::as_str).unwrap_or("").trim().to_string();
                let lower = ing.to_lowercase();
                let entry = ingredient_map.entry(lower).or_insert_with(|| (ing.to_string(), Vec::new()));
                if !meas.is_empty() {
                    entry.1.push(meas);
                }
            }
        }
    }

    if ingredient_map.is_empty() {
        return Ok("No ingredients found.".to_string());
    }

    let mut categories: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for (ing_lower, (display_name, measures)) in &ingredient_map {
        let merged = merge_measures(measures);
        let cat = categorize_ingredient(ing_lower);
        categories.entry(cat.to_string()).or_default().push((display_name.clone(), merged));
    }

    let mut out = String::new();
    let order = ["Produce", "Dairy", "Meat", "Bakery", "Pantry", "Other"];
    for cat in &order {
        if let Some(items) = categories.get_mut(*cat) {
            if items.is_empty() { continue; }
            items.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
            out.push_str(&format!("{}:\n", cat));
            for (ing, meas) in items {
                if meas.is_empty() {
                    out.push_str(&format!("  - {}\n", ing));
                } else {
                    out.push_str(&format!("  - {}: {}\n", ing, meas));
                }
            }
            out.push('\n');
        }
    }

    Ok(out.trim_end().to_string())
}

fn categorize_ingredient(ingredient: &str) -> &'static str {
    let lower = ingredient;
    const PRODUCE: &[&str] = &["onion", "garlic", "tomato", "potato", "carrot", "lettuce", "cucumber",
        "pepper", "mushroom", "lemon", "lime", "avocado", "spinach", "broccoli", "corn", "celery",
        "ginger", "herb", "basil", "parsley", "cilantro", "coriander", "mint", "rosemary", "thyme",
        "oregano", "chili", "chilli", "scallion", "shallot", "green bean", "pea", "zucchini",
        "courgette", "aubergine", "eggplant", "cabbage", "kale", "radish", "beet", "turnip", "parsnip"];
    const DAIRY: &[&str] = &["milk", "cream", "butter", "cheese", "yogurt", "yoghurt", "egg",
        "margarine", "buttermilk", "sour cream", "cream cheese", "ricotta", "mozzarella",
        "parmesan", "cheddar", "feta"];
    const MEAT: &[&str] = &["chicken", "beef", "pork", "lamb", "turkey", "duck", "bacon", "ham",
        "sausage", "prosciutto", "salami", "mince", "steak", "chop", "roast", "fish", "salmon",
        "tuna", "cod", "shrimp", "prawn", "crab", "lobster"];
    const BAKERY: &[&str] = &["flour", "bread", "pasta", "rice", "noodle", "tortilla", "wrap",
        "bun", "roll", "pastry", "dough", "crust", "oats", "cereal"];
    const PANTRY: &[&str] = &["oil", "sauce", "soy", "vinegar", "stock", "broth", "spice",
        "pepper", "salt", "sugar", "honey", "syrup", "jam", "peanut butter", "ketchup",
        "mustard", "mayo", "canned", "tomato paste", "coconut milk", "bouillon", "worcestershire"];

    for kw in PRODUCE { if lower.contains(kw) { return "Produce"; } }
    for kw in DAIRY { if lower.contains(kw) { return "Dairy"; } }
    for kw in MEAT { if lower.contains(kw) { return "Meat"; } }
    for kw in BAKERY { if lower.contains(kw) { return "Bakery"; } }
    for kw in PANTRY { if lower.contains(kw) { return "Pantry"; } }
    "Other"
}

fn merge_measures(measures: &[String]) -> String {
    let non_empty: Vec<&str> = measures.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect();
    if non_empty.is_empty() { return String::new(); }
    if non_empty.len() == 1 { return non_empty[0].to_string(); }

    let mut by_unit: HashMap<String, f64> = HashMap::new();
    let mut can_merge = true;

    for m in &non_empty {
        if let Some((num_str, unit)) = m.split_once(' ') {
            if let Ok(num) = num_str.parse::<f64>() {
                *by_unit.entry(unit.to_string()).or_default() += num;
                continue;
            }
        }
        can_merge = false;
        break;
    }

    if can_merge {
        let mut parts: Vec<String> = by_unit.iter()
            .map(|(unit, total)| {
                if (total - total.round()).abs() < 1e-10 {
                    format!("{} {}", *total as i64, unit)
                } else {
                    format!("{:.1} {}", total, unit)
                }
            })
            .collect();
        parts.sort();
        parts.join(", ")
    } else {
        non_empty.join(", ")
    }
}
