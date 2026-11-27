//! rpe-next 曲线轨迹（`trajectory`）里用到的数字表达式求值。
//!
//! 语法与 rpe-next 的 `compileExpression`（`src/core/batch-script.mjs`）一致，见文件末尾的
//! 优先级表：它只用来算几个坐标，不需要变量赋值、字符串与自定义函数。
//!
//! 解析与求值都返回 `Option`：谱面里的表达式写坏了就退回直线插值，不当作加载失败。

use rustc_hash::FxHashMap;

const MAX_LEN: usize = 4096;
const MAX_TOKENS: usize = 512;

#[derive(Clone, Copy, PartialEq)]
enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
}

#[derive(Clone)]
enum Node {
    Num(f32),
    Var(String),
    Neg(Box<Node>),
    Not(Box<Node>),
    Bin(Op, Box<Node>, Box<Node>),
    Call(String, Vec<Node>),
    Cond(Box<Node>, Box<Node>, Box<Node>),
}

/// 一条已经编译好的表达式。
#[derive(Clone)]
pub struct Expr {
    root: Node,
}

impl Expr {
    /// 解析失败（语法不认识、函数名未知）返回 None。
    pub fn parse(src: &str) -> Option<Self> {
        let src = normalise(src);
        if src.len() > MAX_LEN {
            return None;
        }
        let tokens = tokenize(&src)?;
        if tokens.len() > MAX_TOKENS {
            return None;
        }
        let mut parser = Parser { tokens, cursor: 0 };
        let root = parser.expression(0)?;
        if parser.cursor != parser.tokens.len() {
            return None;
        }
        Some(Self { root })
    }

    pub fn eval(&self, scope: &FxHashMap<String, f32>) -> Option<f32> {
        let value = eval_node(&self.root, scope)?;
        value.is_finite().then_some(value)
    }
}

/// rpe-next 允许中文逗号、`π`、`θ`、`Pi` 这些写法，先统一成 ASCII。
fn normalise(src: &str) -> String {
    src.replace("$t$", "t")
        .replace('θ', "theta")
        .replace("Pi", "pi")
        .replace('π', "pi")
        .replace('，', ",")
        .replace('（', "(")
        .replace('）', ")")
}

#[derive(Clone, PartialEq)]
enum Token {
    Num(f32),
    Ident(String),
    Sym(&'static str),
}

fn tokenize(src: &str) -> Option<Vec<Token>> {
    let bytes: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c.is_ascii_digit() || c == '.' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == '.') {
                i += 1;
            }
            // 指数部分：1e-3
            if i < bytes.len() && (bytes[i] == 'e' || bytes[i] == 'E') {
                let mut j = i + 1;
                if j < bytes.len() && (bytes[j] == '+' || bytes[j] == '-') {
                    j += 1;
                }
                if j < bytes.len() && bytes[j].is_ascii_digit() {
                    i = j;
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            let text: String = bytes[start..i].iter().collect();
            out.push(Token::Num(text.parse().ok()?));
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == '_' || bytes[i] == '.') {
                i += 1;
            }
            out.push(Token::Ident(bytes[start..i].iter().collect()));
            continue;
        }
        let rest: String = bytes[i..bytes.len().min(i + 2)].iter().collect();
        let sym = match rest.as_str() {
            "**" => Some("**"),
            "&&" => Some("&&"),
            "||" => Some("||"),
            "==" => Some("=="),
            "!=" => Some("!="),
            "<=" => Some("<="),
            ">=" => Some(">="),
            _ => match c {
                '+' => Some("+"),
                '-' => Some("-"),
                '*' => Some("*"),
                '/' => Some("/"),
                '%' => Some("%"),
                '^' => Some("^"),
                '<' => Some("<"),
                '>' => Some(">"),
                '!' => Some("!"),
                '(' => Some("("),
                ')' => Some(")"),
                ',' => Some(","),
                ':' => Some(":"),
                '?' => Some("?"),
                _ => None,
            },
        }?;
        out.push(Token::Sym(sym));
        i += if rest.len() == 2 && (sym == "**" || sym == "&&" || sym == "||" || sym == "==" || sym == "!=" || sym == "<=" || sym == ">=") {
            2
        } else {
            1
        };
    }
    Some(out)
}

/// 二元运算符的结合性与优先级，数字越大越紧。
fn precedence(op: &str) -> Option<(u8, bool)> {
    match op {
        "||" => Some((1, false)),
        "&&" => Some((2, false)),
        "==" | "!=" => Some((3, false)),
        "<" | ">" | "<=" | ">=" => Some((4, false)),
        "+" | "-" => Some((5, false)),
        "*" | "/" | "%" => Some((6, false)),
        // `^` 与 `**` 右结合
        "^" | "**" => Some((7, true)),
        _ => None,
    }
}

struct Parser {
    tokens: Vec<Token>,
    cursor: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.cursor)
    }

    fn next(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.cursor).cloned();
        self.cursor += 1;
        t
    }

    fn eat(&mut self, sym: &str) -> bool {
        if let Some(Token::Sym(s)) = self.peek() {
            if *s == sym {
                self.cursor += 1;
                return true;
            }
        }
        false
    }

    fn atom(&mut self) -> Option<Node> {
        match self.next()? {
            Token::Num(v) => Some(Node::Num(v)),
            Token::Sym("+") => self.atom(),
            Token::Sym("-") => Some(Node::Neg(Box::new(self.atom()?))),
            Token::Sym("!") => Some(Node::Not(Box::new(self.atom()?))),
            Token::Sym("(") => {
                let node = self.expression(0)?;
                if !self.eat(")") {
                    return None;
                }
                Some(node)
            }
            Token::Ident(name) => {
                if !matches!(self.peek(), Some(Token::Sym("("))) {
                    return Some(Node::Var(name));
                }
                self.cursor += 1;
                let mut args = Vec::new();
                if !matches!(self.peek(), Some(Token::Sym(")"))) {
                    loop {
                        args.push(self.expression(0)?);
                        if !self.eat(",") {
                            break;
                        }
                    }
                }
                if !self.eat(")") {
                    return None;
                }
                Some(Node::Call(name, args))
            }
            _ => None,
        }
    }

    fn expression(&mut self, minimum: u8) -> Option<Node> {
        let mut left = self.atom()?;
        while let Some(Token::Sym(sym)) = self.peek().cloned() {
            // 不是运算符就结束（`)`、`,`、`:` 都会走到这里），不能当成解析失败。
            let Some((prec, right_assoc)) = precedence(sym) else {
                break;
            };
            if prec < minimum {
                break;
            }
            self.cursor += 1;
            let next_min = if right_assoc { prec } else { prec + 1 };
            let right = self.expression(next_min)?;
            let op = to_op(sym)?;
            left = Node::Bin(op, Box::new(left), Box::new(right));
        }
        if minimum == 0 && self.eat("?") {
            let yes = self.expression(0)?;
            if !self.eat(":") {
                return None;
            }
            let no = self.expression(0)?;
            left = Node::Cond(Box::new(left), Box::new(yes), Box::new(no));
        }
        Some(left)
    }
}

fn to_op(sym: &str) -> Option<Op> {
    Some(match sym {
        "+" => Op::Add,
        "-" => Op::Sub,
        "*" => Op::Mul,
        "/" => Op::Div,
        "%" => Op::Rem,
        "^" | "**" => Op::Pow,
        "==" => Op::Eq,
        "!=" => Op::Ne,
        "<" => Op::Lt,
        ">" => Op::Gt,
        "<=" => Op::Le,
        ">=" => Op::Ge,
        "&&" => Op::And,
        "||" => Op::Or,
        _ => return None,
    })
}

fn eval_node(node: &Node, scope: &FxHashMap<String, f32>) -> Option<f32> {
    Some(match node {
        Node::Num(v) => *v,
        Node::Var(name) => match name.as_str() {
            "pi" => std::f32::consts::PI,
            "true" => 1.,
            "false" => 0.,
            _ => *scope.get(name)?,
        },
        Node::Neg(inner) => -eval_node(inner, scope)?,
        Node::Not(inner) => (eval_node(inner, scope)? == 0.) as u8 as f32,
        Node::Cond(c, yes, no) => {
            if eval_node(c, scope)? != 0. {
                eval_node(yes, scope)?
            } else {
                eval_node(no, scope)?
            }
        }
        Node::Bin(op, a, b) => {
            let a = eval_node(a, scope)?;
            // `&&` / `||` 短路，与 rpe-next 一致
            if *op == Op::And {
                return if a != 0. { Some((eval_node(b, scope)? != 0.) as u8 as f32) } else { Some(0.) };
            }
            if *op == Op::Or {
                return if a != 0. { Some(1.) } else { Some((eval_node(b, scope)? != 0.) as u8 as f32) };
            }
            let b = eval_node(b, scope)?;
            match op {
                Op::Add => a + b,
                Op::Sub => a - b,
                Op::Mul => a * b,
                Op::Div => a / b,
                Op::Rem => a % b,
                Op::Pow => a.powf(b),
                Op::Eq => (a == b) as u8 as f32,
                Op::Ne => (a != b) as u8 as f32,
                Op::Lt => (a < b) as u8 as f32,
                Op::Gt => (a > b) as u8 as f32,
                Op::Le => (a <= b) as u8 as f32,
                Op::Ge => (a >= b) as u8 as f32,
                Op::And | Op::Or => unreachable!(),
            }
        }
        Node::Call(name, args) => {
            let mut v = Vec::with_capacity(args.len());
            for arg in args {
                v.push(eval_node(arg, scope)?);
            }
            call(name, &v)?
        }
    })
}

/// 内建函数表，与 rpe-next `batch-script.mjs` 的 `functions` 一一对应。
fn call(name: &str, args: &[f32]) -> Option<f32> {
    let a = *args.first()?;
    Some(match name {
        "abs" => a.abs(),
        "floor" => a.floor(),
        "ceil" => a.ceil(),
        "round" => a.round(),
        "sin" => a.sin(),
        "cos" => a.cos(),
        "tan" => a.tan(),
        "sqrt" => a.sqrt(),
        "min" => args.iter().copied().fold(f32::INFINITY, f32::min),
        "max" => args.iter().copied().fold(f32::NEG_INFINITY, f32::max),
        "pow" => a.powf(*args.get(1)?),
        "clamp" => a.max(*args.get(1)?).min(*args.get(2)?),
        "lerp" => a + (*args.get(1)? - a) * args.get(2)?,
        "ease" => {
            // rpe-next 的 `ease(progress, type)` 用的是事件那套 1..30 的缓动编号。
            let id = args.get(1).copied().unwrap_or(1.).clamp(1., 30.) as usize;
            let tween = super::RPE_TWEEN_MAP[id - 1] as usize;
            crate::core::TWEEN_FUNCTIONS[tween](a)
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(src: &str, scope: &[(&str, f32)]) -> Option<f32> {
        let map: FxHashMap<String, f32> = scope.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        Expr::parse(src)?.eval(&map)
    }

    #[test]
    fn basic_arithmetic() {
        assert_eq!(eval("1 + 2 * 3", &[]), Some(7.));
        assert_eq!(eval("2 ^ 3 ^ 2", &[]), Some(512.), "幂是右结合");
        assert_eq!(eval("(1 + 2) * 3", &[]), Some(9.));
        assert_eq!(eval("-2 * -3", &[]), Some(6.));
    }

    #[test]
    fn variables_and_functions() {
        let scope = [("t", 0.25f32), ("radius", 200.)];
        let v = eval("radius * cos(2 * pi * t)", &scope).unwrap();
        assert!((v - 0.).abs() < 1e-3, "{v}");
        assert_eq!(eval("clamp(5, 0, 1)", &[]), Some(1.));
        assert_eq!(eval("lerp(0, 10, 0.5)", &[]), Some(5.));
        assert_eq!(eval("max(abs(-3), 2)", &[]), Some(3.));
    }

    #[test]
    fn unknown_things_are_none() {
        assert!(Expr::parse("1 +").is_none());
        assert!(eval("unknown(1)", &[]).is_none());
        assert!(eval("nope * 2", &[]).is_none());
    }
}
