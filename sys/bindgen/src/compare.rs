use proc_macro2::Span;
use quote::quote;
use std::collections::HashSet;
use std::error::Error;
use std::fs;
use std::path::Path;
use syn::punctuated::{Pair, Punctuated};
use syn::visit_mut;
use syn::visit_mut::VisitMut;
use syn::{AngleBracketedGenericArguments, Fields, Ident, Item, Type, TypeBareFn};

type DynError = Box<dyn Error>;

pub enum CmpResult {
    Equivalent,
    Different(CmpDiff),
}

pub struct CmpDiff {
    pub file1: Vec<String>,
    pub file2: Vec<String>,
}

// this function allow to ignore some i32/u32 difference
fn i32_to_u32(mut item: Item) -> Item {
    match &mut item {
        Item::Type(it) => {
            if let Type::Path(tp) = it.ty.as_mut() {
                for e in tp.path.segments.iter_mut() {
                    if format!("{}", e.ident) == "i32" {
                        e.ident = Ident::new("u32", Span::call_site());
                    }
                }
            }
        }
        Item::Struct(item_struct) => {
            if let Fields::Unnamed(ref mut fu) = item_struct.fields {
                for field in fu.unnamed.iter_mut() {
                    if let Type::Path(ref mut tp) = field.ty {
                        for e in tp.path.segments.iter_mut() {
                            if format!("{}", e.ident) == "i32" {
                                e.ident = Ident::new("u32", Span::call_site());
                            }
                        }
                    }
                }
            }
        }
        _ => (),
    }
    item
}

//visitor that remove trailing comma
struct FnRemoveComma;

impl VisitMut for FnRemoveComma {
    fn visit_type_bare_fn_mut(&mut self, node: &mut TypeBareFn) {
        node.inputs.remove_comma();

        // Delegate to the default impl to visit any nested functions.
        visit_mut::visit_type_bare_fn_mut(self, node);
    }
    fn visit_angle_bracketed_generic_arguments_mut(
        &mut self,
        node: &mut AngleBracketedGenericArguments,
    ) {
        node.args.remove_comma();
        visit_mut::visit_angle_bracketed_generic_arguments_mut(self, node);
    }
    fn visit_item_mut(&mut self, node: &mut Item) {
        //println!("Item with name={:#?}", node);

        // Delegate to the default impl to visit any nested functions.
        visit_mut::visit_item_mut(self, node);
    }
}

trait RemoveComma {
    fn remove_comma(&mut self);
}

impl<T, P> RemoveComma for Punctuated<T, P>
where
    P: Default,
{
    fn remove_comma(&mut self) {
        if let Some(pair) = self.pop() {
            match pair {
                Pair::Punctuated(t, _p) => self.push(t),
                Pair::End(t) => self.push(t),
            }
        }
    }
}

fn remove_comma(mut item: Item) -> Item {
    FnRemoveComma.visit_item_mut(&mut item);
    item
}

// the idea here is to represent a binding file as a unordonned collection of syn::Item. This to be
// insensitive to formatting or definition order in the file.
pub fn compare(file1: &Path, file2: &Path) -> Result<CmpResult, DynError> {
    let f1 = fs::read_to_string(file1)?;
    let f1 = syn::parse_str::<syn::File>(&f1)?;
    let h1: HashSet<_> = f1
        .items
        .into_iter()
        .map(remove_comma)
        .map(i32_to_u32)
        .collect();

    let f2 = fs::read_to_string(file2)?;
    let f2 = syn::parse_str::<syn::File>(&f2)?;
    let h2: HashSet<_> = f2
        .items
        .into_iter()
        .map(remove_comma)
        .map(i32_to_u32)
        .collect();

    if h1 != h2 {
        let diff1: Vec<String> = h1
            .difference(&h2)
            .map(|item| quote!(#item).to_string())
            .collect();
        let diff2: Vec<String> = h2
            .difference(&h1)
            .map(|item| quote!(#item).to_string())
            .collect();

        Ok(CmpResult::Different(CmpDiff {
            file1: diff1,
            file2: diff2,
        }))
    } else {
        Ok(CmpResult::Equivalent)
    }
}
