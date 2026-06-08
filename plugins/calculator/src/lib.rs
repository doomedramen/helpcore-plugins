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

struct Calculator;

impl Guest for Calculator {
    fn call(tool: String, input_json: String) -> Result<String, String> {
        let input: Value = serde_json::from_str(&input_json)
            .map_err(|e| format!("invalid input JSON: {e}"))?;

        match tool.as_str() {
            "calc" => calc(&input),
            _ => Err(format!("unknown tool: {tool}")),
        }
    }
}

export!(Calculator);

// ── Expression evaluator ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    Percent,
    LParen,
    RParen,
    Comma,
    Ident(String),
}

struct Lexer {
    chars: Vec<char>,
    pos: usize,
}

impl Lexer {
    fn new(input: &str) -> Self {
        Lexer {
            chars: input.chars().collect(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.chars.get(self.pos).copied();
        self.pos += 1;
        ch
    }

    fn skip_whitespace(&mut self) {
        while let Some(ch) = self.peek() {
            if ch.is_whitespace() {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn read_number(&mut self) -> f64 {
        let mut s = String::new();
        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() || ch == '.' {
                s.push(ch);
                self.advance();
            } else {
                break;
            }
        }
        // Support scientific notation: 1e5, 2.5e-3
        if self.peek() == Some('e') || self.peek() == Some('E') {
            s.push(self.advance().unwrap());
            if self.peek() == Some('-') || self.peek() == Some('+') {
                s.push(self.advance().unwrap());
            }
            while let Some(ch) = self.peek() {
                if ch.is_ascii_digit() {
                    s.push(ch);
                    self.advance();
                } else {
                    break;
                }
            }
        }
        s.parse().unwrap_or(0.0)
    }

    fn read_ident(&mut self) -> String {
        let mut s = String::new();
        while let Some(ch) = self.peek() {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                s.push(ch);
                self.advance();
            } else {
                break;
            }
        }
        s
    }

    fn next_token(&mut self) -> Option<Token> {
        self.skip_whitespace();
        let ch = self.peek()?;
        match ch {
            '0'..='9' => Some(Token::Number(self.read_number())),
            '+' => {
                self.advance();
                Some(Token::Plus)
            }
            '-' => {
                self.advance();
                Some(Token::Minus)
            }
            '*' => {
                self.advance();
                Some(Token::Star)
            }
            '/' => {
                self.advance();
                Some(Token::Slash)
            }
            '^' => {
                self.advance();
                Some(Token::Caret)
            }
            '%' => {
                self.advance();
                Some(Token::Percent)
            }
            '(' => {
                self.advance();
                Some(Token::LParen)
            }
            ')' => {
                self.advance();
                Some(Token::RParen)
            }
            ',' => {
                self.advance();
                Some(Token::Comma)
            }
            c if c.is_ascii_alphabetic() => {
                let ident = self.read_ident();
                match ident.as_str() {
                    "pi" => Some(Token::Number(std::f64::consts::PI)),
                    "e" => Some(Token::Number(std::f64::consts::E)),
                    "sqrt" | "sin" | "cos" | "tan" | "abs" | "log" | "ln"
                    | "exp" | "floor" | "ceil" | "round" => Some(Token::Ident(ident)),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

struct Parser {
    lexer: Lexer,
    current: Option<Token>,
}

impl Parser {
    fn new(input: &str) -> Self {
        let mut lexer = Lexer::new(input);
        let current = lexer.next_token();
        Parser { lexer, current }
    }

    fn advance(&mut self) {
        self.current = self.lexer.next_token();
    }

    /// expression := term (('+' | '-') term)*
    fn expression(&mut self) -> Result<f64, String> {
        let mut left = self.term()?;
        loop {
            match &self.current {
                Some(Token::Plus) => {
                    self.advance();
                    left += self.term()?;
                }
                Some(Token::Minus) => {
                    self.advance();
                    left -= self.term()?;
                }
                _ => break,
            }
        }
        Ok(left)
    }

    /// term := factor (('*' | '/' | '%') factor)*
    fn term(&mut self) -> Result<f64, String> {
        let mut left = self.factor()?;
        loop {
            match &self.current {
                Some(Token::Star) => {
                    self.advance();
                    left *= self.factor()?;
                }
                Some(Token::Slash) => {
                    self.advance();
                    let right = self.factor()?;
                    if right == 0.0 {
                        return Err("division by zero".to_string());
                    }
                    left /= right;
                }
                Some(Token::Percent) => {
                    self.advance();
                    left %= self.factor()?;
                }
                _ => break,
            }
        }
        Ok(left)
    }

    /// factor := unary (('^') unary)*   (right-associative)
    fn factor(&mut self) -> Result<f64, String> {
        let left = self.unary()?;
        if matches!(self.current, Some(Token::Caret)) {
            self.advance();
            let right = self.factor()?;
            Ok(left.powf(right))
        } else {
            Ok(left)
        }
    }

    /// unary := ('+' | '-')? atom
    fn unary(&mut self) -> Result<f64, String> {
        match &self.current {
            Some(Token::Plus) => {
                self.advance();
                self.unary()
            }
            Some(Token::Minus) => {
                self.advance();
                self.unary().map(|v| -v)
            }
            _ => self.atom(),
        }
    }

    /// atom := number | '(' expression ')' | function '(' arguments ')'
    fn atom(&mut self) -> Result<f64, String> {
        match self.current.take() {
            Some(Token::Number(n)) => {
                self.advance();
                Ok(n)
            }
            Some(Token::LParen) => {
                self.advance();
                let val = self.expression()?;
                match &self.current {
                    Some(Token::RParen) => {
                        self.advance();
                        Ok(val)
                    }
                    _ => Err("expected closing parenthesis ')'".to_string()),
                }
            }
            Some(Token::Ident(func)) => {
                self.advance();
                match self.current {
                    Some(Token::LParen) => {
                        self.advance();
                        let args = self.arguments()?;
                        match self.current {
                            Some(Token::RParen) => {
                                self.advance();
                                eval_function(&func, &args)
                            }
                            _ => Err(format!("expected ')' after {func} arguments")),
                        }
                    }
                    _ => Err(format!("expected '(' after function '{func}'")),
                }
            }
            Some(other) => Err(format!("unexpected token: {other:?}")),
            None => Err("unexpected end of expression".to_string()),
        }
    }

    /// arguments := expression (',' expression)*
    fn arguments(&mut self) -> Result<Vec<f64>, String> {
        let mut args = vec![self.expression()?];
        while matches!(self.current, Some(Token::Comma)) {
            self.advance();
            args.push(self.expression()?);
        }
        Ok(args)
    }
}

fn eval_function(name: &str, args: &[f64]) -> Result<f64, String> {
    if args.is_empty() {
        return Err(format!("function '{name}' requires at least 1 argument"));
    }
    let x = args[0];
    match name {
        "sqrt" => {
            if x < 0.0 {
                return Err("sqrt of negative number".to_string());
            }
            Ok(x.sqrt())
        }
        "sin" => Ok(x.sin()),
        "cos" => Ok(x.cos()),
        "tan" => Ok(x.tan()),
        "abs" => Ok(x.abs()),
        "log" => {
            if x <= 0.0 {
                return Err("log of non-positive number".to_string());
            }
            Ok(x.log10())
        }
        "ln" => {
            if x <= 0.0 {
                return Err("ln of non-positive number".to_string());
            }
            Ok(x.ln())
        }
        "exp" => Ok(x.exp()),
        "floor" => Ok(x.floor()),
        "ceil" => Ok(x.ceil()),
        "round" => Ok(x.round()),
        _ => Err(format!("unknown function: {name}")),
    }
}

fn format_result(val: f64) -> String {
    if val.is_nan() {
        return "NaN".to_string();
    }
    if val.is_infinite() {
        return if val > 0.0 {
            "Infinity".to_string()
        } else {
            "-Infinity".to_string()
        };
    }
    // Use enough precision, then trim trailing zeros
    let s = format!("{:.15}", val);
    let s = s.trim_end_matches('0');
    let s = s.trim_end_matches('.');
    if s.is_empty() {
        "0".to_string()
    } else {
        s.to_string()
    }
}

fn calc(input: &Value) -> Result<String, String> {
    let expr = input
        .get("expression")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or("expression is required, e.g. '2 + 3 * 4'")?;

    let mut parser = Parser::new(expr);
    let result = parser.expression().map_err(|e| format!("parse error: {e}"))?;

    // Check for trailing garbage
    if parser.current.is_some() {
        return Err(format!(
            "unexpected input after expression",
        ));
    }

    Ok(format_result(result))
}
