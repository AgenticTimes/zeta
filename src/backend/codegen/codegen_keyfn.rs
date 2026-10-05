// src/backend/codegen/codegen_keyfn.rs
// 批 1000（轴 D 第二刀）：keyfn 家族从 codegen.rs 迁出——
// verify_keyfn_contract（W0911 双层签名核对）＋FuncAddr 兜底臂
// （__ZKEYF64_ 特化副本地址铸造）。语义零变，纯位置迁移（迁前/迁后
// LLVM IR 基线逐字节 diff 为空，迁出纪律同 gen.rs 主题块模式）。

use super::codegen::LLVMCodegen;
use super::signature_table;
use crate::middle::mir::mir::Mir;
use inkwell::values::FunctionValue;
use inkwell::values::BasicValueEnum;

impl<'ctx> LLVMCodegen<'ctx> {
    /// 批 990（提案①第一段）：keyfn 特化副本的签名合同核对（W0911）。
    /// 核对两层——
    /// 1. `fn_val` 的实际 LLVM 签名：副本可能已被 FuncAddr 兜底按
    ///    double(f64) 抢先声明，体发射进一个错签名的 FunctionValue 会
    ///    静默产出错域指令；
    /// 2. 体派生签名：param 槽型（type_map）＋`infer_fn_return_type`——
    ///    批 983 的病根（参数注解没生效 ⇒ 槽 I64 ⇒ 体 i64(i64)）在这一
    ///    层现形。
    /// 任一层偏离 `KEYFN_PTR_CONTRACT`（double(f64)）即 panic：错签名过
    /// C 桥的产物是运行期静默垃圾，宁可编译期响亮失败。
    pub(super) fn verify_keyfn_contract(
        &self,
        name: &str,
        fn_val: FunctionValue<'ctx>,
        mir: &Mir,
    ) {
        let contract = signature_table::KEYFN_PTR_CONTRACT;
        // 1) 实际声明层
        let ft = fn_val.get_type();
        let actual_ret = ft
            .get_return_type()
            .map(Self::val_ty_of_basic)
            .unwrap_or(signature_table::ValTy::I64);
        let actual_params: Vec<signature_table::ValTy> = ft
            .get_param_types()
            .iter()
            .map(|t| Self::val_ty_of_metadata(t))
            .collect();
        if !signature_table::sig_matches(actual_ret, &actual_params, contract) {
            panic!(
                "W0911 keyfn 签名核对失败: `{}` 实际声明 {}({})，C 桥合同 double(f64)——特化副本声明漂移（批 990 签名表核对）",
                name,
                Self::val_ty_name(actual_ret),
                actual_params
                    .iter()
                    .map(|t| Self::val_ty_name(*t))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        // 2) 体派生层
        let derived_params: Vec<signature_table::ValTy> = mir
            .param_indices
            .iter()
            .map(|(_, pid)| signature_table::val_ty_of(mir.type_map.get(pid)))
            .collect();
        let derived_ret = Self::val_ty_of_basic(self.infer_fn_return_type(mir));
        if !signature_table::sig_matches(derived_ret, &derived_params, contract) {
            panic!(
                "W0911 keyfn 签名核对失败: `{}` 体派生签名 {}({})，C 桥合同 double(f64)——参数注解 f64 未生效或返回域不是浮点（批 983 病根，批 990 响亮化）",
                name,
                Self::val_ty_name(derived_ret),
                derived_params
                    .iter()
                    .map(|t| Self::val_ty_name(*t))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }

    /// 批 967/997：FuncAddr 兜底臂的 keyfn 分支——`__ZKEYF64_` 特化副本
    /// 的兜底声明与真体**同签名** double(f64)（副本参数注解 f64 ⇒
    /// codegen 参数签名 f64、返回 double；同名实体复用）。C 侧
    /// py_max_key_f64 把元素位模式 bitcast 成 double 传入（i64 元素走
    /// py_max_key_i64_f64 的 sitofp 桥——兜底签名与两桥的 keyfn 指针型
    /// 一致，均为 double(f64)）。批 989：三连重复块并一。
    /// Some ＝已处理（调用方直接返回该值）；None＝不可能是 keyfn 名
    ///（调用方已用前缀判定，此处实际恒 Some）。
    pub(super) fn try_funcaddr_keyfn_addr(
        &mut self,
        name: &str,
    ) -> Option<BasicValueEnum<'ctx>> {
        let f = match self.module.get_function(name) {
            Some(f) => f,
            None => self.module.add_function(
                name,
                self.f64_type
                    .fn_type(&[self.f64_type.into()], false),
                Some(inkwell::module::Linkage::External),
            ),
        };
        let fptr = f.as_global_value().as_pointer_value();
        Some(
            self.builder
                .build_ptr_to_int(fptr, self.i64_type, "keyfn_addr")
                .unwrap()
                .into(),
        )
    }

    /// LLVM 值类型 → ABI 值类别。指针同槽归 I64；浮点按位宽分列。
    fn val_ty_of_basic(t: inkwell::types::BasicTypeEnum<'ctx>) -> signature_table::ValTy {
        match t {
            inkwell::types::BasicTypeEnum::IntType(_) => signature_table::ValTy::I64,
            inkwell::types::BasicTypeEnum::FloatType(ft) => {
                if ft.get_bit_width() == 64 {
                    signature_table::ValTy::F64
                } else {
                    signature_table::ValTy::F32
                }
            }
            _ => signature_table::ValTy::I64,
        }
    }

    /// 实参位（BasicMetadataTypeEnum）版——inkwell 的 fn 实参类型枚举。
    fn val_ty_of_metadata(
        t: &inkwell::types::BasicMetadataTypeEnum<'ctx>,
    ) -> signature_table::ValTy {
        match t {
            inkwell::types::BasicMetadataTypeEnum::IntType(_) => {
                signature_table::ValTy::I64
            }
            inkwell::types::BasicMetadataTypeEnum::FloatType(f) => {
                if f.get_bit_width() == 64 {
                    signature_table::ValTy::F64
                } else {
                    signature_table::ValTy::F32
                }
            }
            _ => signature_table::ValTy::I64,
        }
    }

    fn val_ty_name(t: signature_table::ValTy) -> &'static str {
        match t {
            signature_table::ValTy::I64 => "i64",
            signature_table::ValTy::F64 => "double",
            signature_table::ValTy::F32 => "float",
        }
    }
}
