//! Constant value representation for CTFE

use super::error::{CtfeError, CtfeResult};

/// A value that can be computed at compile time
#[derive(Debug, Clone, PartialEq)]
pub enum ConstValue {
    /// Signed 64-bit integer
    Int(i64),
    /// Unsigned 64-bit integer
    UInt(u64),
    /// Boolean value
    Bool(bool),
    /// Array of constant values
    Array(Vec<ConstValue>),
    /// Compact array of integers (no per-element enum overhead)
    IntArray(Vec<i64>),
    /// Unit type (empty tuple)
    Unit,
    /// String literal (the content, e.g. "/tmp/data")
    String(String),
}

impl ConstValue {
    /// Get the type name of this value
    pub fn type_name(&self) -> &'static str {
        match self {
            ConstValue::Int(_) => "i64",
            ConstValue::UInt(_) => "u64",
            ConstValue::Bool(_) => "bool",
            ConstValue::Array(_) => "array",
            ConstValue::IntArray(_) => "int_array",
            ConstValue::Unit => "unit",
            ConstValue::String(_) => "str",
        }
    }

    /// Try to convert to bool
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            ConstValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Try to convert to signed integer
    pub fn as_int(&self) -> Option<i64> {
        match self {
            ConstValue::Int(n) => Some(*n),
            _ => None,
        }
    }

    /// Try to convert to unsigned integer
    pub fn as_uint(&self) -> Option<u64> {
        match self {
            ConstValue::UInt(n) => Some(*n),
            _ => None,
        }
    }

    /// Try to convert to array
    pub fn as_array(&self) -> Option<&Vec<ConstValue>> {
        match self {
            ConstValue::Array(arr) => Some(arr),
            _ => None,
        }
    }

    /// Get element at index in an array. Returns error if not array or index OOB.
    pub fn array_index(&self, index: usize) -> CtfeResult<&ConstValue> {
        match self {
            ConstValue::Array(elements) => {
                if index < elements.len() {
                    Ok(&elements[index])
                } else {
                    Err(CtfeError::IndexOutOfBounds {
                        index,
                        length: elements.len(),
                    })
                }
            }
            ConstValue::IntArray(elements) => {
                if index < elements.len() {
                    // Leak a Box<ConstValue> to get a &'static reference.
                    // Safe in single-threaded CTFE; caller clones immediately.
                    let leaked = Box::leak(Box::new(ConstValue::Int(elements[index])));
                    Ok(leaked)
                } else {
                    Err(CtfeError::IndexOutOfBounds {
                        index,
                        length: elements.len(),
                    })
                }
            }
            _ => Err(CtfeError::TypeMismatch {
                expected: "array".to_string(),
                found: self.type_name().to_string(),
            }),
        }
    }

    /// Get an i64 element from an IntArray by cloning the value out.
    /// Used when you can own the result.
    pub fn intarray_get(&self, index: usize) -> CtfeResult<ConstValue> {
        match self {
            ConstValue::IntArray(elements) => {
                if index < elements.len() {
                    Ok(ConstValue::Int(elements[index]))
                } else {
                    Err(CtfeError::IndexOutOfBounds {
                        index,
                        length: elements.len(),
                    })
                }
            }
            _ => Err(CtfeError::TypeMismatch {
                expected: "int_array".to_string(),
                found: self.type_name().to_string(),
            }),
        }
    }

    /// Set element at index in an array. Mutates in-place. Returns error if not array or index OOB.
    pub fn array_set(&mut self, index: usize, value: ConstValue) -> CtfeResult<()> {
        match self {
            ConstValue::Array(elements) => {
                if index < elements.len() {
                    elements[index] = value;
                    Ok(())
                } else {
                    Err(CtfeError::IndexOutOfBounds {
                        index,
                        length: elements.len(),
                    })
                }
            }
            ConstValue::IntArray(elements) => {
                if index < elements.len() {
                    // Attempt to store an Int value into the compact array
                    match value {
                        ConstValue::Int(i) => {
                            elements[index] = i;
                            Ok(())
                        }
                        _ => Err(CtfeError::TypeMismatch {
                            expected: "int (i64)".to_string(),
                            found: value.type_name().to_string(),
                        }),
                    }
                } else {
                    Err(CtfeError::IndexOutOfBounds {
                        index,
                        length: elements.len(),
                    })
                }
            }
            _ => Err(CtfeError::TypeMismatch {
                expected: "array".to_string(),
                found: self.type_name().to_string(),
            }),
        }
    }

    /// Return the length of the array (works for both Array and IntArray)
    pub fn array_len(&self) -> Option<usize> {
        match self {
            ConstValue::Array(elements) => Some(elements.len()),
            ConstValue::IntArray(elements) => Some(elements.len()),
            _ => None,
        }
    }

    /// Perform a binary operation
    pub fn binary_op(&self, op: &str, right: &Self) -> CtfeResult<Self> {
        match (self, right) {
            (ConstValue::Int(left_val), ConstValue::Int(right_val)) => {
                match Self::compare_int(*left_val, op, *right_val) {
                    Some(cmp) => Ok(ConstValue::Bool(cmp)),
                    None => Self::binary_op_int(*left_val, op, *right_val).map(ConstValue::Int),
                }
            }
            (ConstValue::UInt(left_val), ConstValue::UInt(right_val)) => {
                match Self::compare_uint(*left_val, op, *right_val) {
                    Some(cmp) => Ok(ConstValue::Bool(cmp)),
                    None => Self::binary_op_uint(*left_val, op, *right_val).map(ConstValue::UInt),
                }
            }
            (ConstValue::Bool(left_val), ConstValue::Bool(right_val)) => {
                Self::binary_op_bool(*left_val, op, *right_val).map(ConstValue::Bool)
            }
            (ConstValue::IntArray(_), _) | (_, ConstValue::IntArray(_)) => {
                Err(CtfeError::InvalidOperation {
                    op: op.to_string(),
                    left_type: self.type_name().to_string(),
                    right_type: Some(right.type_name().to_string()),
                })
            }
            _ => Err(CtfeError::InvalidOperation {
                op: op.to_string(),
                left_type: self.type_name().to_string(),
                right_type: Some(right.type_name().to_string()),
            }),
        }
    }

    /// Perform a unary operation
    pub fn unary_op(&self, op: &str) -> CtfeResult<Self> {
        match self {
            ConstValue::Int(val) => Self::unary_op_int(*val, op).map(ConstValue::Int),
            ConstValue::UInt(val) => Self::unary_op_uint(*val, op).map(ConstValue::UInt),
            ConstValue::Bool(val) => Self::unary_op_bool(*val, op).map(ConstValue::Bool),
            ConstValue::IntArray(_) => Err(CtfeError::InvalidOperation {
                op: op.to_string(),
                left_type: self.type_name().to_string(),
                right_type: None,
            }),
            _ => Err(CtfeError::InvalidOperation {
                op: op.to_string(),
                left_type: self.type_name().to_string(),
                right_type: None,
            }),
        }
    }

    /// Try to convert to IntArray reference
    pub fn as_int_array(&self) -> Option<&Vec<i64>> {
        match self {
            ConstValue::IntArray(arr) => Some(arr),
            _ => None,
        }
    }

    /// Try to convert to IntArray mutable reference
    pub fn as_int_array_mut(&mut self) -> Option<&mut Vec<i64>> {
        match self {
            ConstValue::IntArray(arr) => Some(arr),
            _ => None,
        }
    }

    /// A comparison yields a **bool**, not a number — in Python `3 == 4` is
    /// `False`, and the fold must not decide otherwise: returning `Int(0)` here
    /// made `print(3 == 4)` print `0`, because the whole `ConstValue::Int` →
    /// `AstNode::Lit` → `Type::I64` → `println_i64` chain downstream has no way
    /// left to recover the bool. Returns `None` for non-comparison operators so
    /// the caller keeps using the arithmetic folder.
    fn compare_int(left: i64, op: &str, right: i64) -> Option<bool> {
        match op {
            "==" => Some(left == right),
            "!=" => Some(left != right),
            "<" => Some(left < right),
            "<=" => Some(left <= right),
            ">" => Some(left > right),
            ">=" => Some(left >= right),
            _ => None,
        }
    }

    /// See [`Self::compare_int`].
    fn compare_uint(left: u64, op: &str, right: u64) -> Option<bool> {
        match op {
            "==" => Some(left == right),
            "!=" => Some(left != right),
            "<" => Some(left < right),
            "<=" => Some(left <= right),
            ">" => Some(left > right),
            ">=" => Some(left >= right),
            _ => None,
        }
    }

    /// Binary operation for signed integers
    fn binary_op_int(left: i64, op: &str, right: i64) -> CtfeResult<i64> {
        match op {
            "+" => Ok(left.wrapping_add(right)),
            "-" => Ok(left.wrapping_sub(right)),
            "*" => Ok(left.wrapping_mul(right)),
            "/" => {
                if right == 0 {
                    Err(CtfeError::DivisionByZero)
                } else {
                    Ok(left.wrapping_div(right))
                }
            }
            "%" => {
                if right == 0 {
                    Err(CtfeError::DivisionByZero)
                } else {
                    Ok(left % right)
                }
            }
            // PY-A: floor division — Python rounds toward negative infinity,
            // `wrapping_div` truncates toward zero.
            "floordiv" => {
                if right == 0 {
                    Err(CtfeError::DivisionByZero)
                } else {
                    let q = left.wrapping_div(right);
                    let r = left % right;
                    Ok(if r != 0 && ((r < 0) != (right < 0)) { q - 1 } else { q })
                }
            }
            "&" => Ok(left & right),
            "|" => Ok(left | right),
            "^" => Ok(left ^ right),
            "<<" => {
                if !(0..64).contains(&right) {
                    Err(CtfeError::Overflow)
                } else {
                    Ok(left.wrapping_shl(right as u32))
                }
            }
            ">>" => {
                if !(0..64).contains(&right) {
                    Err(CtfeError::Overflow)
                } else {
                    Ok(left.wrapping_shr(right as u32))
                }
            }
            _ => Err(CtfeError::UnsupportedOperation(format!(
                "binary operator '{}' for integers",
                op
            ))),
        }
    }

    /// Binary operation for unsigned integers
    fn binary_op_uint(left: u64, op: &str, right: u64) -> CtfeResult<u64> {
        match op {
            "+" => Ok(left.wrapping_add(right)),
            "-" => Ok(left.wrapping_sub(right)),
            "*" => Ok(left.wrapping_mul(right)),
            "/" => {
                if right == 0 {
                    Err(CtfeError::DivisionByZero)
                } else {
                    Ok(left.wrapping_div(right))
                }
            }
            "%" => {
                if right == 0 {
                    Err(CtfeError::DivisionByZero)
                } else {
                    Ok(left % right)
                }
            }
            // PY-A: floor division — unsigned operands are already floored by
            // truncation, so this is just the plain quotient.
            "floordiv" => {
                if right == 0 {
                    Err(CtfeError::DivisionByZero)
                } else {
                    Ok(left / right)
                }
            }
            "&" => Ok(left & right),
            "|" => Ok(left | right),
            "^" => Ok(left ^ right),
            "<<" => Ok(left.wrapping_shl(right as u32)),
            ">>" => Ok(left.wrapping_shr(right as u32)),
            _ => Err(CtfeError::UnsupportedOperation(format!(
                "binary operator '{}' for unsigned integers",
                op
            ))),
        }
    }

    /// Binary operation for booleans
    fn binary_op_bool(left: bool, op: &str, right: bool) -> CtfeResult<bool> {
        match op {
            "==" => Ok(left == right),
            "!=" => Ok(left != right),
            "&&" => Ok(left && right),
            "||" => Ok(left || right),
            _ => Err(CtfeError::UnsupportedOperation(format!(
                "binary operator '{}' for booleans",
                op
            ))),
        }
    }

    /// Unary operation for signed integers
    fn unary_op_int(val: i64, op: &str) -> CtfeResult<i64> {
        match op {
            "-" => Ok(val.wrapping_neg()),
            "!" => Ok(!val),
            _ => Err(CtfeError::UnsupportedOperation(format!(
                "unary operator '{}' for integers",
                op
            ))),
        }
    }

    /// Unary operation for unsigned integers
    fn unary_op_uint(val: u64, op: &str) -> CtfeResult<u64> {
        match op {
            "!" => Ok(!val),
            _ => Err(CtfeError::UnsupportedOperation(format!(
                "unary operator '{}' for unsigned integers",
                op
            ))),
        }
    }

    /// Unary operation for booleans
    fn unary_op_bool(val: bool, op: &str) -> CtfeResult<bool> {
        match op {
            "!" => Ok(!val),
            _ => Err(CtfeError::UnsupportedOperation(format!(
                "unary operator '{}' for booleans",
                op
            ))),
        }
    }
}
