use crate::dom_arena::NodeId;

#[derive(Debug, Clone, PartialEq)]
pub enum JsValue {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    Str(String),
    Object(u32),
    Array(u32),
    DomNode(NodeId),
    Fn(u32),
    Regex(u32),
    Map(u32),
    Set(u32),
    TypedArray(u32),
}

impl JsValue {
    pub fn as_str(&self) -> String {
        match self {
            JsValue::Undefined => "undefined".to_string(),
            JsValue::Null => "null".to_string(),
            JsValue::Bool(b) => b.to_string(),
            JsValue::Number(n) => {
                if n.is_nan() {
                    "NaN".to_string()
                } else if n.is_infinite() {
                    if *n > 0.0 { "Infinity" } else { "-Infinity" }.to_string()
                } else if n.fract() == 0.0 && n.abs() < 1e15 {
                    format!("{:.0}", n)
                } else {
                    n.to_string()
                }
            }
            JsValue::Str(s) => s.clone(),
            JsValue::Object(_) => "[object Object]".to_string(),
            JsValue::Array(_) => "[object Array]".to_string(),
            JsValue::DomNode(id) => format!("[object Element#{}]", id),
            JsValue::Fn(_) => "function() { [manta bytecode] }".to_string(),
            JsValue::Regex(_) => "/RegExp/".to_string(),
            JsValue::Map(_) => "[object Map]".to_string(),
            JsValue::Set(_) => "[object Set]".to_string(),
            JsValue::TypedArray(_) => "[object Float64Array]".to_string(),
        }
    }

    pub fn as_number(&self) -> f64 {
        match self {
            JsValue::Undefined => f64::NAN,
            JsValue::Null | JsValue::Bool(false) => 0.0,
            JsValue::Bool(true) => 1.0,
            JsValue::Number(n) => *n,
            JsValue::Str(s) => {
                let t = s.trim();
                if t.is_empty() {
                    0.0
                } else {
                    t.parse::<f64>().unwrap_or(f64::NAN)
                }
            }
            _ => f64::NAN,
        }
    }

    pub fn as_i32(&self) -> i32 {
        let n = self.as_number();
        if n.is_nan() || n.is_infinite() || n == 0.0 {
            0
        } else {
            n.trunc() as i64 as i32
        }
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            JsValue::Undefined | JsValue::Null => false,
            JsValue::Bool(b) => *b,
            JsValue::Number(n) => *n != 0.0 && !n.is_nan(),
            JsValue::Str(s) => !s.is_empty(),
            _ => true,
        }
    }

    pub fn type_of(&self) -> &'static str {
        match self {
            JsValue::Undefined => "undefined",
            JsValue::Null => "object",
            JsValue::Bool(_) => "boolean",
            JsValue::Number(_) => "number",
            JsValue::Str(_) => "string",
            JsValue::Fn(_) => "function",
            _ => "object",
        }
    }
}

#[derive(Debug, Clone)]
pub struct JsParam {
    pub name_id: u16,
    pub default_pc: Option<usize>,
    pub is_rest: bool,
}

#[derive(Debug, Clone)]
pub struct JsFunctionDef {
    pub name: Option<String>,
    pub params: Vec<JsParam>,
    pub entry_pc: usize,
    pub is_async: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OpCode {
    LoadConst(u16),
    LoadIdent(u16),
    DeclVar(u16),
    StoreVar(u16),
    MakeArray(u16),
    ArrayPush,
    ArraySpread,
    MakeObject(u16),
    ObjectSpread,
    MakeClosure(u16),
    MakeRegex { pat_id: u16, flags_id: u16 },
    BuildClass { has_parent: bool, method_count: u16 },
    GetProp(u16),
    GetPropOpt(u16),
    SetProp(u16),
    DeleteProp(u16),
    GetIndex,
    SetIndex,
    DestructureObj { pairs: Vec<(u16, u16, bool)> },
    DestructureArr { targets: Vec<(u16, bool)> },
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    BitAnd,
    BitOr,
    BitXor,
    BitNot,
    Shl,
    Shr,
    UShr,
    Neg,
    Not,
    TypeOf,
    InOp,
    InstanceOf,
    Eq,
    Neq,
    StrictEq,
    StrictNeq,
    Lt,
    Lte,
    Gt,
    Gte,
    And,
    Or,
    Nullish,
    Jump(usize),
    JumpIfFalse(usize),
    JumpIfTrue(usize),
    PushTry(usize),
    PopTry,
    Throw,
    Await,
    Construct { argc: u8 },
    CallValue { argc: u8 },
    CallValueArray,
    CallMethod { prop_id: u16, argc: u8 },
    CallMethodArray { prop_id: u16 },
    KeysArray,
    Return,
    Pop,
    Dup,
    Halt,
}

#[derive(Debug, Clone, Default)]
pub struct CompiledJsProgram {
    pub constants: Vec<JsValue>,
    pub identifiers: Vec<String>,
    pub functions: Vec<JsFunctionDef>,
    pub instructions: Vec<OpCode>,
}

impl CompiledJsProgram {
    pub(crate) fn add_const(&mut self, val: JsValue) -> u16 {
        if let Some(pos) = self.constants.iter().position(|c| c == &val) {
            return pos as u16;
        }
        let idx = self.constants.len() as u16;
        self.constants.push(val);
        idx
    }

    pub(crate) fn add_ident(&mut self, name: &str) -> u16 {
        if let Some(pos) = self.identifiers.iter().position(|i| i == name) {
            return pos as u16;
        }
        let idx = self.identifiers.len() as u16;
        self.identifiers.push(name.to_string());
        idx
    }
}

pub(crate) fn abstract_eq(a: &JsValue, b: &JsValue) -> bool {
    if std::mem::discriminant(a) == std::mem::discriminant(b) {
        return a == b;
    }
    match (a, b) {
        (JsValue::Null, JsValue::Undefined) | (JsValue::Undefined, JsValue::Null) => true,
        (JsValue::Number(n), JsValue::Str(_)) | (JsValue::Str(_), JsValue::Number(n)) => {
            *n == b.as_number() && *n == a.as_number()
        }
        (JsValue::Bool(_), _) | (_, JsValue::Bool(_)) => a.as_number() == b.as_number(),
        _ => false,
    }
}
