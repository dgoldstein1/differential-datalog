use std::cmp::Ordering;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Formatter;
use std::hash::Hash;
use std::hash::Hasher;
use std::result::Result;

use serde::de::Error;
use serde::Deserializer;
use serde::Serializer;

use differential_datalog::ddval::DDVal;
use differential_datalog::ddval::DDValue;

/* This module is designed to be imported both as a standard DDlog library and as a normal Rust
 * module, e.g., from `differential_datalog_test`.  We therefore need to import thit trait
 * so that it is available in the latter case and rename it so that it doesn't cause duplicate
 * import error in the former case. */
use differential_datalog::record::IntoRecord as IntoRec;
use differential_datalog::record::Record;
use ordered_float::OrderedFloat;

use abomonation::Abomonation;

/// All DDlog types are expected to implement this trait.  In particular, it is used as a type
/// bound on all type variables.
pub trait Val:
    Default
    + Eq
    + Ord
    + Clone
    + Hash
    + PartialEq
    + PartialOrd
    + serde::Serialize
    + ::serde::de::DeserializeOwned
    + 'static
{
}

impl<T> Val for T where
    T: Default
        + Eq
        + Ord
        + Clone
        + Hash
        + PartialEq
        + PartialOrd
        + serde::Serialize
        + ::serde::de::DeserializeOwned
        + 'static
{
}

/// Use in generated Rust code to implement string concatenation (`++`)
pub fn string_append_str(mut s1: String, s2: &str) -> String {
    s1.push_str(s2);
    s1
}

/// Use in generated Rust code to implement string concatenation (`++`)
#[allow(clippy::ptr_arg)]
pub fn string_append(mut s1: String, s2: &String) -> String {
    s1.push_str(s2.as_str());
    s1
}

/// Used to implement fields with `deserialize_from_array` attributed.
#[macro_export]
macro_rules! deserialize_map_from_array {
    ( $modname:ident, $ktype:ty, $vtype:ty, $kfunc:path ) => {
        mod $modname {
            use super::*;
            use serde::de::{Deserialize, Deserializer};
            use serde::ser::Serializer;
            use std::collections::BTreeMap;

            pub fn serialize<S>(
                map: &crate::ddlog_std::Map<$ktype, $vtype>,
                serializer: S,
            ) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.collect_seq(map.x.values())
            }

            pub fn deserialize<'de, D>(
                deserializer: D,
            ) -> Result<crate::ddlog_std::Map<$ktype, $vtype>, D::Error>
            where
                D: Deserializer<'de>,
            {
                let v = Vec::<$vtype>::deserialize(deserializer)?;
                Ok(v.into_iter().map(|item| ($kfunc(&item), item)).collect())
            }
        }
    };
}

/* Runtime support for DDlog closures. */

/* DDlog's equivalent of Rust's `Fn` trait.  This is necessary, as Rust does not allow manual
 * implementations of `Fn` trait (until `unboxed_closures` and `fn_traits` features are
 * stabilized).  Otherwise, we would just derive `Fn` and add methods for comparison and hashing.
 */
pub trait Closure<Args, Output> {
    fn call(&self, args: Args) -> Output;
    /* Returns pointers to function and captured arguments, for use in comparison methods. */
    fn internals(&self) -> (usize, usize);
    fn clone_dyn(&self) -> Box<dyn Closure<Args, Output>>;
    fn eq_dyn(&self, other: &dyn Closure<Args, Output>) -> bool;
    fn cmp_dyn(&self, other: &dyn Closure<Args, Output>) -> Ordering;
    fn hash_dyn(&self, state: &mut dyn Hasher);
    fn into_record_dyn(&self) -> Record;
    fn fmt_debug_dyn(&self, f: &mut Formatter) -> std::fmt::Result;
    fn fmt_display_dyn(&self, f: &mut Formatter) -> std::fmt::Result;
    fn serialize_dyn(&self) -> &dyn erased_serde::Serialize;
}

#[derive(Clone)]
pub struct ClosureImpl<Args, Output, Captured: Val> {
    pub description: &'static str,
    pub captured: Captured,
    pub f: fn(args: Args, captured: &Captured) -> Output,
}

impl<Args, Output, Captured: Debug + Val> serde::Serialize for ClosureImpl<Args, Output, Captured> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&format!(
            "<closure: {}, captured_args: {:?}>",
            self.description, self.captured
        ))
    }
}

/* Rust forces 'static trait bound on `Args` and `Output`, as the borrow checker is not smart
 * enough to realize that they are only used as arguments to `f`.
 */
impl<Args: Clone + 'static, Output: Clone + 'static, Captured: Debug + Val> Closure<Args, Output>
    for ClosureImpl<Args, Output, Captured>
{
    fn call(&self, args: Args) -> Output {
        (self.f)(args, &self.captured)
    }

    fn clone_dyn(&self) -> Box<dyn Closure<Args, Output>> {
        Box::new((*self).clone()) as Box<dyn Closure<Args, Output>>
    }

    fn internals(&self) -> (usize, usize) {
        (
            self.f as *const (fn(Args, &Captured) -> Output) as usize,
            &self.captured as *const Captured as usize,
        )
    }

    fn eq_dyn(&self, other: &dyn Closure<Args, Output>) -> bool {
        /* Compare function pointers.  If equal, it is safe to compare captured variables. */
        let (other_f, other_captured) = other.internals();
        if (other_f == (self.f as *const (fn(Args, &Captured) -> Output) as usize)) {
            unsafe { *(other_captured as *const Captured) == self.captured }
        } else {
            false
        }
    }

    fn cmp_dyn(&self, other: &dyn Closure<Args, Output>) -> Ordering {
        let (other_f, other_captured) = other.internals();
        match ((self.f as *const (fn(Args, &Captured) -> Output) as usize).cmp(&other_f)) {
            Ordering::Equal => self
                .captured
                .cmp(unsafe { &*(other_captured as *const Captured) }),
            ord => ord,
        }
    }

    fn hash_dyn(&self, mut state: &mut dyn Hasher) {
        self.captured.hash(&mut state);
        (self.f as *const (fn(Args, &Captured) -> Output) as usize).hash(&mut state);
    }

    fn into_record_dyn(&self) -> Record {
        Record::String(format!(
            "<closure: {}, captured_args: {:?}>",
            self.description, self.captured
        ))
    }

    fn fmt_debug_dyn(&self, f: &mut Formatter) -> std::fmt::Result {
        f.write_fmt(format_args!(
            "<closure: {}, captured_args: {:?}>",
            self.description, self.captured
        ))
    }

    fn fmt_display_dyn(&self, f: &mut Formatter) -> std::fmt::Result {
        f.write_fmt(format_args!(
            "<closure: {}, captured_args: {:?}>",
            self.description, self.captured
        ))
    }

    fn serialize_dyn(&self) -> &dyn erased_serde::Serialize {
        self as &dyn erased_serde::Serialize
    }
}

impl<Args: Clone + 'static, Output: Clone + 'static> Display for Box<dyn Closure<Args, Output>> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.fmt_display_dyn(f)
    }
}

impl<Args: Clone + 'static, Output: Clone + 'static> Debug for Box<dyn Closure<Args, Output>> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.fmt_debug_dyn(f)
    }
}

impl<Args: Clone + 'static, Output: Clone + 'static> PartialEq<&Self>
    for Box<dyn Closure<Args, Output>>
{
    fn eq(&self, other: &&Self) -> bool {
        self.eq_dyn(&***other)
    }
}

/* This extra impl is a workaround for compiler bug that fails to derive `PartialEq` for
 * structs that contain fields of type `Box<dyn Closure<>>`. See:
 * https://github.com/rust-lang/rust/issues/31740#issuecomment-700950186 */
impl<Args: Clone + 'static, Output: Clone + 'static> PartialEq for Box<dyn Closure<Args, Output>> {
    fn eq(&self, other: &Self) -> bool {
        self.eq_dyn(&**other)
    }
}
impl<Args: Clone + 'static, Output: Clone + 'static> Eq for Box<dyn Closure<Args, Output>> {}

impl<Args: Clone + 'static, Output: Clone + 'static> PartialOrd for Box<dyn Closure<Args, Output>> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp_dyn(&**other))
    }
}
impl<Args: Clone + 'static, Output: Clone + 'static> Ord for Box<dyn Closure<Args, Output>> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.cmp_dyn(&**other)
    }
}

impl<Args: Clone + 'static, Output: Clone + 'static> Clone for Box<dyn Closure<Args, Output>> {
    fn clone(&self) -> Self {
        self.clone_dyn()
    }
}

impl<Args: 'static + Clone, Output: 'static + Clone + Default> Default
    for Box<dyn Closure<Args, Output>>
{
    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn default() -> Self {
        Box::new(ClosureImpl {
            description: "default closure",
            captured: (),
            f: {
                fn __f<A, O: Default>(args: A, captured: &()) -> O {
                    O::default()
                };
                __f
            },
        })
    }
}

impl<Args: 'static + Clone, Output: 'static + Clone> Hash for Box<dyn Closure<Args, Output>> {
    fn hash<H>(&self, state: &mut H)
    where
        H: Hasher,
    {
        self.hash_dyn(state);
    }
}

impl<Args: 'static + Clone, Output: 'static + Clone> serde::Serialize
    for Box<dyn Closure<Args, Output>>
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        erased_serde::serialize((self.serialize_dyn()), serializer)
    }
}

impl<'de, Args: 'static + Clone, Output: 'static + Clone> serde::Deserialize<'de>
    for Box<dyn Closure<Args, Output>>
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Err(D::Error::custom(
            "Deserialization of closures is not implemented.",
        ))
    }
}

impl<Args: 'static + Clone, Output: 'static + Clone>
    differential_datalog::record::Mutator<Box<dyn Closure<Args, Output>>> for Record
{
    fn mutate(&self, x: &mut Box<dyn Closure<Args, Output>>) -> Result<(), String> {
        Err("'mutate' not implemented for closures.".to_string())
    }
}

impl<Args: 'static + Clone, Output: 'static + Clone> differential_datalog::record::IntoRecord
    for Box<dyn Closure<Args, Output>>
{
    fn into_record(self) -> Record {
        self.into_record_dyn()
    }
}

impl<Args: 'static + Clone, Output: 'static + Clone> differential_datalog::record::FromRecord
    for Box<dyn Closure<Args, Output>>
{
    fn from_record(val: &Record) -> Result<Self, String> {
        Err("'from_record' not implemented for closures.".to_string())
    }
}

impl<Args: 'static + Clone, Output: 'static + Clone> Abomonation
    for Box<dyn Closure<Args, Output>>
{
    unsafe fn entomb<W: std::io::Write>(&self, _write: &mut W) -> std::io::Result<()> {
        panic!("Closure::entomb: not implemented")
    }
    unsafe fn exhume<'a, 'b>(&'a mut self, _bytes: &'b mut [u8]) -> Option<&'b mut [u8]> {
        panic!("Closure::exhume: not implemented")
    }
    fn extent(&self) -> usize {
        panic!("Closure::extent: not implemented")
    }
}

#[cfg(test)]
mod tests {
    use super::Closure;
    use super::ClosureImpl;
    use serde::Deserialize;
    use serde::Serialize;

    #[test]
    fn closure_test() {
        let closure1: ClosureImpl<(*const String, *const u32), Vec<String>, Vec<u64>> =
            ClosureImpl {
                description: "test closure 1",
                captured: vec![0, 1, 2, 3],
                f: {
                    fn __f(args: (*const String, *const u32), captured: &Vec<u64>) -> Vec<String> {
                        captured
                            .iter()
                            .map(|x| {
                                format!(
                                    "x: {}, arg0: {}, arg1: {}",
                                    x,
                                    unsafe { &*args.0 },
                                    unsafe { &*args.1 }
                                )
                            })
                            .collect()
                    };
                    __f
                },
            };

        let closure2: ClosureImpl<(*const String, *const u32), Vec<String>, String> = ClosureImpl {
            description: "test closure 1",
            captured: "Bar".to_string(),
            f: {
                fn __f(args: (*const String, *const u32), captured: &String) -> Vec<String> {
                    vec![format!(
                        "captured: {}, arg0: {}, arg1: {}",
                        captured,
                        unsafe { &*args.0 },
                        unsafe { &*args.1 }
                    )]
                };
                __f
            },
        };

        let ref arg1 = "bar".to_string();
        let ref arg2: u32 = 100;
        assert_eq!(
            closure1.call((arg1, arg2)),
            vec![
                "x: 0, arg0: bar, arg1: 100",
                "x: 1, arg0: bar, arg1: 100",
                "x: 2, arg0: bar, arg1: 100",
                "x: 3, arg0: bar, arg1: 100"
            ]
        );
        assert!(closure1.eq_dyn(&*closure1.clone_dyn()));
        assert!(closure2.eq_dyn(&*closure2.clone_dyn()));
        assert_eq!(closure1.eq_dyn(&closure2), false);
    }

    /* Make sure that auto-derives work for closures. */

    #[derive(Eq, PartialEq, Ord, Clone, Hash, PartialOrd, Default, Serialize, Deserialize)]
    pub struct IntClosure {
        pub f: Box<dyn Closure<*const i64, i64>>,
    }

    #[derive(Eq, PartialEq, Ord, Clone, Hash, PartialOrd, Serialize, Deserialize)]
    pub enum ClosureEnum {
        Enum1 {
            f: Box<dyn Closure<*const i64, i64>>,
        },
        Enum2 {
            f: Box<dyn Closure<(*mut Vec<String>, *const IntClosure), ()>>,
        },
    }
}

/// `trait DDValConvert` must be implemented by any type that supports conversion to/from `DDValue`
/// representation.
pub trait DDValConvert: Sized {
    /// Extract reference to concrete type from `&DDVal`.  This causes undefined behavior
    /// if `v` does not contain a value of type `Self`.
    unsafe fn from_ddval_ref(v: &DDVal) -> &Self;

    unsafe fn from_ddvalue_ref(v: &DDValue) -> &Self {
        Self::from_ddval_ref(&v.val)
    }

    /// Extracts concrete value contained in `v`.  Panics if `v` does not contain a
    /// value of type `Self`.
    unsafe fn from_ddval(v: DDVal) -> Self;

    unsafe fn from_ddvalue(v: DDValue) -> Self {
        Self::from_ddval(v.into_ddval())
    }

    /// Convert a value to a `DDVal`, erasing its original type.  This is a safe conversion
    /// that cannot fail.
    fn into_ddval(self) -> DDVal;

    fn ddvalue(&self) -> DDValue;
    fn into_ddvalue(self) -> DDValue;
}

/// Macro to implement `DDValConvert` for type `t` that satisfies the following type bounds:
///
/// t: Eq + Ord + Clone + Send + Debug + Sync + Hash + PartialOrd + IntoRecord + 'static,
/// Record: Mutator<t>
///
#[macro_export]
macro_rules! decl_ddval_convert {
    ( $t:ty ) => {
        impl $crate::ddlog_rt::DDValConvert for $t {
            unsafe fn from_ddval_ref(v: &differential_datalog::ddval::DDVal) -> &Self {
                if ::std::mem::size_of::<Self>() <= ::std::mem::size_of::<usize>() {
                    &*(&v.v as *const usize as *const Self)
                } else {
                    &*(v.v as *const Self)
                }
            }

            unsafe fn from_ddval(v: differential_datalog::ddval::DDVal) -> Self {
                if ::std::mem::size_of::<Self>() <= ::std::mem::size_of::<usize>() {
                    let res: Self =
                        ::std::mem::transmute::<[u8; ::std::mem::size_of::<Self>()], Self>(
                            *(&v.v as *const usize as *const [u8; ::std::mem::size_of::<Self>()]),
                        );
                    ::std::mem::forget(v);
                    res
                } else {
                    let arc = ::std::sync::Arc::from_raw(v.v as *const Self);
                    ::std::sync::Arc::try_unwrap(arc).unwrap_or_else(|a| (*a).clone())
                }
            }

            fn into_ddval(self) -> differential_datalog::ddval::DDVal {
                if ::std::mem::size_of::<Self>() <= ::std::mem::size_of::<usize>() {
                    let mut v: usize = 0;
                    unsafe {
                        *(&mut v as *mut usize as *mut [u8; ::std::mem::size_of::<Self>()]) =
                            ::std::mem::transmute::<Self, [u8; ::std::mem::size_of::<Self>()]>(
                                self,
                            );
                    };
                    differential_datalog::ddval::DDVal { v }
                } else {
                    differential_datalog::ddval::DDVal {
                        v: ::std::sync::Arc::into_raw(::std::sync::Arc::new(self)) as usize,
                    }
                }
            }

            fn ddvalue(&self) -> differential_datalog::ddval::DDValue {
                $crate::ddlog_rt::DDValConvert::into_ddvalue(self.clone())
            }

            fn into_ddvalue(self) -> differential_datalog::ddval::DDValue {
                const VTABLE: differential_datalog::ddval::DDValMethods =
                    differential_datalog::ddval::DDValMethods {
                        clone: {
                            fn __f(
                                this: &differential_datalog::ddval::DDVal,
                            ) -> differential_datalog::ddval::DDVal {
                                if ::std::mem::size_of::<$t>() <= ::std::mem::size_of::<usize>() {
                                    unsafe { <$t>::from_ddval_ref(this) }.clone().into_ddval()
                                } else {
                                    let arc =
                                        unsafe { ::std::sync::Arc::from_raw(this.v as *const $t) };
                                    let res = differential_datalog::ddval::DDVal {
                                        v: ::std::sync::Arc::into_raw(arc.clone()) as usize,
                                    };
                                    ::std::sync::Arc::into_raw(arc);
                                    res
                                }
                            };
                            __f
                        },
                        into_record: {
                            fn __f(
                                this: differential_datalog::ddval::DDVal,
                            ) -> differential_datalog::record::Record {
                                unsafe { <$t>::from_ddval(this) }.into_record()
                            };
                            __f
                        },
                        eq: {
                            fn __f(
                                this: &differential_datalog::ddval::DDVal,
                                other: &differential_datalog::ddval::DDVal,
                            ) -> bool {
                                unsafe {
                                    <$t>::from_ddval_ref(this).eq(<$t>::from_ddval_ref(other))
                                }
                            };
                            __f
                        },
                        partial_cmp: {
                            fn __f(
                                this: &differential_datalog::ddval::DDVal,
                                other: &differential_datalog::ddval::DDVal,
                            ) -> Option<::std::cmp::Ordering> {
                                unsafe {
                                    <$t>::from_ddval_ref(this)
                                        .partial_cmp(<$t>::from_ddval_ref(other))
                                }
                            };
                            __f
                        },
                        cmp: {
                            fn __f(
                                this: &differential_datalog::ddval::DDVal,
                                other: &differential_datalog::ddval::DDVal,
                            ) -> ::std::cmp::Ordering {
                                unsafe {
                                    <$t>::from_ddval_ref(this).cmp(<$t>::from_ddval_ref(other))
                                }
                            };
                            __f
                        },
                        hash: {
                            fn __f(
                                this: &differential_datalog::ddval::DDVal,
                                mut state: &mut dyn std::hash::Hasher,
                            ) {
                                ::std::hash::Hash::hash(
                                    unsafe { <$t>::from_ddval_ref(this) },
                                    &mut state,
                                );
                            };
                            __f
                        },
                        mutate: {
                            fn __f(
                                this: &mut differential_datalog::ddval::DDVal,
                                record: &differential_datalog::record::Record,
                            ) -> Result<(), ::std::string::String> {
                                let mut clone = unsafe { <$t>::from_ddval_ref(this) }.clone();
                                differential_datalog::record::Mutator::mutate(record, &mut clone)?;
                                *this = clone.into_ddval();
                                Ok(())
                            };
                            __f
                        },
                        fmt_debug: {
                            fn __f(
                                this: &differential_datalog::ddval::DDVal,
                                f: &mut ::std::fmt::Formatter,
                            ) -> Result<(), ::std::fmt::Error> {
                                ::std::fmt::Debug::fmt(unsafe { <$t>::from_ddval_ref(this) }, f)
                            };
                            __f
                        },
                        fmt_display: {
                            fn __f(
                                this: &differential_datalog::ddval::DDVal,
                                f: &mut ::std::fmt::Formatter,
                            ) -> Result<(), ::std::fmt::Error> {
                                ::std::fmt::Display::fmt(
                                    &unsafe { <$t>::from_ddval_ref(this) }.clone().into_record(),
                                    f,
                                )
                            };
                            __f
                        },
                        drop: {
                            fn __f(this: &mut differential_datalog::ddval::DDVal) {
                                if ::std::mem::size_of::<$t>() <= ::std::mem::size_of::<usize>() {
                                    unsafe {
                                        let _v: $t = ::std::mem::transmute::<
                                            [u8; ::std::mem::size_of::<$t>()],
                                            $t,
                                        >(
                                            *(&this.v as *const usize
                                                as *const [u8; ::std::mem::size_of::<$t>()]),
                                        );
                                    };
                                // v's destructor will do the rest.
                                } else {
                                    let _arc =
                                        unsafe { ::std::sync::Arc::from_raw(this.v as *const $t) };
                                    // arc's destructor will do the rest.
                                }
                            };
                            __f
                        },
                        ddval_serialize: {
                            fn __f(
                                this: &differential_datalog::ddval::DDVal,
                            ) -> &dyn erased_serde::Serialize {
                                (unsafe { <$t>::from_ddval_ref(this) })
                                    as &dyn erased_serde::Serialize
                            };
                            __f
                        },
                    };
                differential_datalog::ddval::DDValue::new(self.into_ddval(), &VTABLE)
            }
        }
    };
}

/* Implement `DDValConvert` for builtin types. */

decl_ddval_convert! {()}
decl_ddval_convert! {u8}
decl_ddval_convert! {u16}
decl_ddval_convert! {u32}
decl_ddval_convert! {u64}
decl_ddval_convert! {u128}
decl_ddval_convert! {i8}
decl_ddval_convert! {i16}
decl_ddval_convert! {i32}
decl_ddval_convert! {i64}
decl_ddval_convert! {i128}
decl_ddval_convert! {String}
decl_ddval_convert! {bool}
decl_ddval_convert! {OrderedFloat<f32>}
decl_ddval_convert! {OrderedFloat<f64>}