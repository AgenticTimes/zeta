//! 批次 891：lower_expr_node 剩余小臂（869 零适配法迁出）。

use super::MirGen;
use crate::frontend::ast::AstNode;
use crate::middle::mir::mir::{MirExpr, MirStmt};
use crate::middle::types::Type;

impl MirGen {

    pub(super) fn lower_bind_pattern(&mut self, name: &String, pattern: &Box<AstNode>, id: u32) -> u32 {
            // Binding pattern: x @ pattern — bind name to inner value.
            let inner_id = self.lower_expr(pattern);
            self.name_to_id.insert(name.clone(), inner_id);
            self.exprs.insert(id, MirExpr::Var(inner_id));
            if let Some(ty) = self.type_map.get(&inner_id) {
                self.type_map.insert(id, ty.clone());
            } else {
                self.type_map.insert(id, Type::I64);
            }
    id
    }

    pub(super) fn lower_range_pattern(
        &mut self,
        start: &Box<AstNode>,
        end: &Box<AstNode>,
        inclusive: &bool,
        id: u32,
    ) -> u32 {
            // Range pattern: lower start/end for comparison.
            let start_id = self.lower_expr(start);
            let end_id = self.lower_expr(end);
            self.exprs.insert(
                id,
                MirExpr::Range {
                    start: start_id,
                    end: end_id,
                },
            );
            self.type_map.insert(id, Type::Range);
    id
    }

    pub(super) fn lower_or_pattern(&mut self, patterns: &Vec<AstNode>, id: u32) -> u32 {
            // Or pattern: evaluate the first alternative.
            if let Some(first) = patterns.first() {
                return self.lower_expr(first);
            }
            self.exprs.insert(id, MirExpr::IntLit(0));
            self.type_map.insert(id, Type::I64);
    id
    }

    pub(super) fn lower_struct_pattern(
        &mut self,
        variant: &String,
        fields: &Vec<(String, AstNode)>,
        rest: &bool,
        id: u32,
    ) -> u32 {
            // Struct pattern in expression position — create struct value.
            let mut field_ids = Vec::new();
            for (field_name, field_expr) in fields {
                let field_id = self.lower_expr(field_expr);
                field_ids.push((field_name.clone(), field_id));
            }
            self.exprs.insert(
                id,
                MirExpr::Struct {
                    variant: variant.clone(),
                    fields: field_ids,
                },
            );
            self.type_map
                .insert(id, Type::Named(variant.clone(), vec![]));
    id
    }

}