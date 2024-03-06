﻿use cuda::{bindings::CUdeviceptr, AsRaw, ContextGuard, DevMem, Module, Ptx, Stream};
use std::{
    ffi::{c_uint, c_void, CString},
    ops::Deref,
};
use tensor::{udim, DataType, Tensor};

pub struct RotaryEmbedding<'ctx> {
    module: Module<'ctx>,
    f: CString,
    block_size: c_uint,
}

impl<'ctx> RotaryEmbedding<'ctx> {
    pub fn new(block_size: usize, ctx: &'ctx ContextGuard<'ctx>) -> Self {
        let name = "rotary_embedding_padding";

        const ROTARY_EMBEDDING: &str = include_str!("rotary_embedding.cuh");
        let code = format!(
            r#"{ROTARY_EMBEDDING}

extern "C" __global__ void {name}(
    half2              *__restrict__ x,
    unsigned int const *__restrict__ pos,
    float theta,
    unsigned int const leading_dim
){{
    padding(x, pos, theta, leading_dim);
}}
"#
        );

        let (ptx, log) = Ptx::compile(code);
        if !log.is_empty() {
            warn!("{log}");
        }
        Self {
            module: ctx.load(&ptx.unwrap()),
            f: CString::new(name).unwrap(),
            block_size: block_size as _,
        }
    }
}

impl RotaryEmbedding<'_> {
    pub fn launch<'a, T>(&self, t: &Tensor<T>, pos: &Tensor<T>, theta: f32, stream: &Stream)
    where
        T: Deref<Target = DevMem<'a>>,
    {
        let &[n, nh, dh] = t.shape() else {
            panic!("Invalid shape");
        };

        assert!(t.contiguous_len() >= 2);
        assert_eq!(t.data_type(), DataType::F16);
        assert_eq!(pos.data_type(), DataType::U32);
        assert_eq!(pos.shape(), &[n]);
        assert!(dh < self.block_size);

        let t_ptr = (unsafe { t.physical().as_raw() } as isize + t.bytes_offset()) as CUdeviceptr;
        let pos_ptr =
            (unsafe { pos.physical().as_raw() } as isize + pos.bytes_offset()) as CUdeviceptr;
        let leading_dim = t.strides()[0] as udim / 2;
        let params: [*const c_void; 4] = [
            (&t_ptr) as *const _ as _,
            (&pos_ptr) as *const _ as _,
            (&theta) as *const _ as _,
            (&leading_dim) as *const _ as _,
        ];

        let f = self.module.get_kernel(&self.f);
        f.launch((nh, n), dh / 2, params.as_ptr(), 0, Some(stream))
    }
}
