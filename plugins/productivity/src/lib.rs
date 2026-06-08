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

struct Productivity;

impl Guest for Productivity {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        seed_rng(&tool, &input_json);
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| format!("invalid input JSON: {e}"))?;

        match tool.as_str() {
            "pomodoro" => pomodoro(&input),
            "random_pick" => random_pick(&input),
            "dice_roll" => dice_roll(&input),
            "coin_flip" => coin_flip(&input),
            "random_number" => random_number(&input),
            "password_generate" => password_generate(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(Productivity);

// ── PRNG ───────────────────────────────────────────────────────────────────────

static mut RNG: u64 = 0;

fn seed_rng(tool: &str, input: &str) {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in tool.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    for b in input.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    unsafe {
        RNG = RNG.wrapping_add(h).wrapping_add(1);
    }
}

fn rng_next() -> u64 {
    unsafe {
        RNG = RNG.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        RNG
    }
}

fn rng_range(lo: u64, hi: u64) -> u64 {
    if hi <= lo {
        return lo;
    }
    lo + (rng_next() % (hi - lo + 1))
}

// ── Helpers ────────────────────────────────────────────────────────────────────

fn get_u64(input: &Value, key: &str, default: u64) -> u64 {
    input.get(key).and_then(|v| v.as_u64()).unwrap_or(default)
}

fn get_i64(input: &Value, key: &str, default: i64) -> i64 {
    input.get(key).and_then(|v| v.as_i64()).unwrap_or(default)
}

fn get_bool(input: &Value, key: &str, default: bool) -> bool {
    input.get(key).and_then(|v| v.as_bool()).unwrap_or(default)
}

fn get_str<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input.get(key).and_then(Value::as_str)
}

// ── Pomodoro ───────────────────────────────────────────────────────────────────

fn pomodoro(input: &Value) -> Result<String, String> {
    let sessions = get_u64(input, "sessions", 4).max(1);
    let work = input
        .get("work")
        .and_then(|v| v.as_u64())
        .or_else(|| input.get("work_minutes").and_then(|v| v.as_u64()))
        .unwrap_or(25)
        .max(1);
    let short_break = get_u64(input, "short_break", 5);
    let long_break = get_u64(input, "long_break", 15);
    let long_after = get_u64(input, "long_break_after", 4).max(1);

    let mut out = String::new();
    let mut total_work = 0u64;
    let mut total_break = 0u64;

    for i in 1u64..=sessions {
        let is_long = i % long_after == 0;
        let (label, break_min) = if is_long {
            ("Long break", long_break)
        } else {
            ("Short break", short_break)
        };
        out.push_str(&format!(
            "#{}: Work {} min → {} {} min\n",
            i, work, label, break_min
        ));
        total_work += work;
        total_break += break_min;
    }

    out.push_str(&format!("\nTotal work:  {} min\n", total_work));
    out.push_str(&format!("Total break: {} min\n", total_break));
    out.push_str(&format!("Grand total: {} min", total_work + total_break));

    Ok(out)
}

// ── Random Pick ────────────────────────────────────────────────────────────────

fn random_pick(input: &Value) -> Result<String, String> {
    let items_str = get_str(input, "items")
        .ok_or("items is required, e.g. 'pizza, sushi, tacos'")?;
    let count = get_u64(input, "count", 1).max(1) as usize;

    let items: Vec<&str> = items_str
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    if items.is_empty() {
        return Err("no items to pick from".into());
    }

    let n = items.len();
    let pick_count = if count >= n { n } else { count };

    let mut picked: Vec<&str> = Vec::with_capacity(pick_count);
    if count >= n {
        // Pick all items in shuffled order
        let mut idx: Vec<usize> = (0..n).collect();
        for i in 0..n {
            let j = i + (rng_next() as usize % (n - i));
            idx.swap(i, j);
            picked.push(items[idx[i]]);
        }
    } else {
        // Pick `count` unique items using Fisher-Yates partial shuffle
        let mut idx: Vec<usize> = (0..n).collect();
        for i in 0..pick_count {
            let j = i + (rng_next() as usize % (n - i));
            idx.swap(i, j);
            picked.push(items[idx[i]]);
        }
    }

    Ok(picked.join(", "))
}

// ── Dice Roll ──────────────────────────────────────────────────────────────────

fn dice_roll(input: &Value) -> Result<String, String> {
    let notation = get_str(input, "notation")
        .ok_or("notation is required, e.g. '1d20' or '3d6'")?;

    let groups: Vec<&str> = notation
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    if groups.is_empty() {
        return Err("no dice notation found".into());
    }

    let mut out = String::new();

    for group in &groups {
        let (count, sides, modifier) = parse_dice(group)?;

        let mut rolls = Vec::with_capacity(count as usize);
        let mut sum: i32 = 0;
        for _ in 0..count {
            let roll = rng_range(1, sides as u64) as i32;
            rolls.push(roll);
            sum += roll;
        }

        if groups.len() > 1 {
            out.push_str(&format!("{}: ", group));
        }

        match (count, modifier) {
            (1, 0) => out.push_str(&format!("{}", sum)),
            (1, m) => out.push_str(&format!("{} {:+} = {}", sum - m, m, sum)),
            (_, 0) => out.push_str(&format!(
                "[{}] = {}",
                rolls.iter().map(i32::to_string).collect::<Vec<_>>().join(", "),
                sum,
            )),
            (_, m) => out.push_str(&format!(
                "[{}] {:+} = {}",
                rolls.iter().map(i32::to_string).collect::<Vec<_>>().join(", "),
                m,
                sum,
            )),
        }

        if groups.len() > 1 {
            out.push('\n');
        }
    }

    Ok(if groups.len() == 1 {
        out
    } else {
        out.trim_end().to_string()
    })
}

fn parse_dice(s: &str) -> Result<(u32, u32, i32), String> {
    let s_lower = s.to_lowercase();
    let d_pos = s_lower.find('d').ok_or_else(|| {
        format!("invalid dice notation '{s}': missing 'd'")
    })?;

    let count: u32 = if d_pos == 0 {
        1
    } else {
        s_lower[..d_pos]
            .parse()
            .map_err(|_| format!("invalid dice count in '{s}'"))?
    };

    let rest = &s_lower[d_pos + 1..];

    let (sides_str, mod_str) = if let Some(p) = rest.find(|c| c == '+' || c == '-') {
        rest.split_at(p)
    } else {
        (rest, "")
    };

    let sides: u32 = sides_str
        .parse()
        .map_err(|_| format!("invalid dice sides in '{s}'"))?;

    let modifier: i32 = if mod_str.is_empty() {
        0
    } else {
        mod_str
            .parse()
            .map_err(|_| format!("invalid modifier in '{s}'"))?
    };

    if count == 0 || sides == 0 {
        return Err(format!("dice count and sides must be positive in '{s}'"));
    }

    Ok((count, sides, modifier))
}

// ── Coin Flip ──────────────────────────────────────────────────────────────────

fn coin_flip(input: &Value) -> Result<String, String> {
    let flips = get_u64(input, "flips", 1).max(1);

    let mut heads = 0u64;
    let mut tails = 0u64;
    let mut results = Vec::with_capacity(flips as usize);

    for _ in 0..flips {
        let is_heads = rng_next() % 2 == 0;
        if is_heads {
            heads += 1;
            results.push("Heads");
        } else {
            tails += 1;
            results.push("Tails");
        }
    }

    let mut out = String::new();
    if flips == 1 {
        out.push_str(results[0]);
    } else {
        for (i, r) in results.iter().enumerate() {
            out.push_str(&format!("#{}: {}\n", i + 1, r));
        }
        out.push_str(&format!("\nHeads: {}\nTails: {}", heads, tails));
    }

    Ok(out)
}

// ── Random Number ──────────────────────────────────────────────────────────────

fn random_number(input: &Value) -> Result<String, String> {
    let min = get_i64(input, "min", 1);
    let max = get_i64(input, "max", 100);
    let count = get_u64(input, "count", 1).max(1) as usize;
    let unique = get_bool(input, "unique", false);

    if min > max {
        return Err("min must be <= max".into());
    }

    let range_size = (max - min + 1) as u64;
    let actual_count = if unique {
        count.min(range_size as usize)
    } else {
        count
    };

    let numbers: Vec<i64> = if unique {
        if range_size <= 100_000 {
            let mut pool: Vec<i64> = (min..=max).collect();
            let n = pool.len();
            for i in 0..actual_count {
                let j = i + (rng_next() as usize % (n - i));
                pool.swap(i, j);
            }
            pool[..actual_count].to_vec()
        } else {
            let mut nums = Vec::with_capacity(actual_count);
            while nums.len() < actual_count {
                let num = min + (rng_next() % range_size) as i64;
                if !nums.contains(&num) {
                    nums.push(num);
                }
            }
            nums
        }
    } else {
        (0..actual_count)
            .map(|_| min + (rng_next() % range_size) as i64)
            .collect()
    };

    Ok(numbers
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", "))
}

// ── Password Generate ──────────────────────────────────────────────────────────

const UPPER: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const LOWER: &str = "abcdefghijklmnopqrstuvwxyz";
const DIGITS: &str = "0123456789";
const SYMBOLS: &str = "!@#$%^&*()-_=+[]{}|;:,.<>?/~`";

fn password_generate(input: &Value) -> Result<String, String> {
    let len = get_u64(input, "length", 20).clamp(1, 128) as usize;
    let upper = get_bool(input, "upper", true);
    let lower = get_bool(input, "lower", true);
    let digits = get_bool(input, "digits", true);
    let symbols = get_bool(input, "symbols", true);

    let mut charset = String::new();
    if upper {
        charset.push_str(UPPER);
    }
    if lower {
        charset.push_str(LOWER);
    }
    if digits {
        charset.push_str(DIGITS);
    }
    if symbols {
        charset.push_str(SYMBOLS);
    }

    if charset.is_empty() {
        return Err("at least one character set must be enabled".into());
    }

    let bytes = charset.as_bytes();
    let mut password = String::with_capacity(len);
    for _ in 0..len {
        let idx = rng_next() as usize % bytes.len();
        password.push(bytes[idx] as char);
    }

    Ok(password)
}
