//! 批次 950：类型声明族发射体（StructDef／EnumDef／ImplBlock／ConceptDef／
//! TypeAlias／Method——869 零适配法自 gen.rs 迁出；原臂逐字）。

use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::r#gen::TypeDecl;

impl MirGen {
    /// StructDef：注册结构类型定义（StructLit / FieldAccess 后续引用）。
    pub(super) fn lower_struct_def(
        &mut self,
        name: &String,
        fields: &Vec<(String, String)>,
        generics: &Vec<crate::frontend::ast::GenericParam>,
    ) {
        // Register struct type definition for later reference by StructLit / FieldAccess.
        self.type_decls.insert(
            name.clone(),
            TypeDecl::Struct {
                fields: fields.clone(),
                generics: generics.clone(),
            },
        );
    }

    /// EnumDef：注册枚举类型定义（模式匹配降级用）。
    pub(super) fn lower_enum_def(
        &mut self,
        name: &String,
        variants: &Vec<(String, Vec<String>)>,
        generics: &Vec<crate::frontend::ast::GenericParam>,
    ) {
        // Register enum type definition for pattern-match lowering.
        self.type_decls.insert(
            name.clone(),
            TypeDecl::Enum {
                variants: variants.clone(),
                generics: generics.clone(),
            },
        );
    }

    /// ImplBlock：降级块内全部 item（函数等）。
    pub(super) fn lower_impl_block(&mut self, ty: &String, body: &Vec<AstNode>) {
        // Lower any items inside the impl block (functions, etc.).
        // BATCH-438: a `class` written inside a function body desugars
        // to [StructDef, ImplBlock, ctor] and this arm is the only
        // place that still holds the class NAME — the methods below are
        // lowered as plain nested `def`s, so without publishing it here
        // `Inner::bump`'s `self` was captured from the env and typed as
        // the ENCLOSING class (its field reads resolved against that
        // layout, declined, and fell through to the `("", 2)` stand-in).
        let outer_class = std::mem::replace(
            &mut self.current_class,
            if ty.is_empty() { None } else { Some(ty.clone()) },
        );
        // BATCH-438: everything this block publishes belongs to THIS
        // window: a second `class _Impl` elsewhere in the program owns a
        // second window with the same key spelling and different symbols.
        let mir_start = self.generated_mirs.len();
        let alias_start = self.nested_class_aliases.len();
        for item in body {
            self.lower_ast(item);
        }
        let aliases = self.nested_class_aliases.split_off(alias_start);
        self.rewrite_nested_class_calls(ty, &aliases, mir_start);
        self.current_class = outer_class;
    }

    /// ConceptDef：降级概念内全部默认方法体。
    pub(super) fn lower_concept_def(&mut self, methods: &Vec<AstNode>) {
        // Lower any default-method bodies inside the concept.
        for method in methods {
            self.lower_ast(method);
        }
    }

    /// TypeAlias：注册类型别名（MIR 层类型解析用）。
    pub(super) fn lower_type_alias(&mut self, name: &String, ty: &String) {
        // Register the type alias so type resolution works at MIR level.
        self.type_decls
            .insert(name.clone(), TypeDecl::Alias { target: ty.clone() });
    }

    /// Method（有体）：降级概念/trait 内的默认方法体。
    pub(super) fn lower_method_body(&mut self, method_body: &Vec<AstNode>) {
        // Lower default method bodies (inside concepts/traits).
        for stmt in method_body {
            self.lower_ast(stmt);
        }
    }
}
