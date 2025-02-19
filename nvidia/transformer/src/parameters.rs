﻿use crate::{Llama2, Tensor};
use common_nv::cuda::{
    ContextGuard, ContextResource, ContextSpore, DevMem, DevMemSpore, EventSpore, Stream,
};

pub(crate) struct ModelParameters {
    model_norm: Tensor<DevMemSpore>,
    lm_head: Tensor<DevMemSpore>,
    sync_event: EventSpore,
}

impl ModelParameters {
    pub fn new(host: &dyn Llama2, stream: &Stream) -> Self {
        macro_rules! map {
            ($param:ident) => {
                host.$param()
                    .as_ref()
                    .map_physical(|slice| stream.from_host(slice).sporulate())
            };
        }
        Self {
            model_norm: map!(model_norm),
            lm_head: map!(lm_head).transpose(&[1, 0]),
            sync_event: stream.record().sporulate(),
        }
    }

    pub unsafe fn release<'ctx>(
        &self,
        stream: &Stream<'ctx>,
    ) -> (Tensor<DevMem<'ctx>>, Tensor<DevMem<'ctx>>) {
        let ctx = stream.ctx();
        stream.wait_for(&self.sync_event.sprout(ctx));
        (
            self.model_norm.as_ref().map_physical(|s| s.sprout(ctx)),
            self.lm_head.as_ref().map_physical(|s| s.sprout(ctx)),
        )
    }

    pub unsafe fn kill(&mut self, ctx: &ContextGuard) {
        self.model_norm.physical_mut().kill(ctx);
        self.lm_head.physical_mut().kill(ctx);
        self.sync_event.kill(ctx);
    }
}

pub(crate) struct LayersParameters {
    layers: Vec<LayerParameter>,
    current: usize,
}

impl LayersParameters {
    pub fn new(load_layers: usize, host: &dyn Llama2, stream: &Stream) -> Self {
        Self {
            layers: (0..host.num_hidden_layers().min(load_layers))
                .map(|layer| LayerParameter::new(host, layer, stream))
                .collect(),
            current: 0,
        }
    }

    #[inline]
    pub fn load(&mut self, layer: usize, host: &dyn Llama2, stream: &Stream) {
        let step = self.layers.len() - 1;
        let i = (self.current + step) % self.layers.len();
        let layer = (layer + step) % host.num_hidden_layers();
        self.layers[i].load(host, layer, stream);
    }

    #[inline]
    pub fn sync(&mut self, layer: usize, stream: &Stream) -> &LayerParameter {
        let i = self.current;
        self.current = (i + 1) % self.layers.len();

        let params = &self.layers[i];
        assert_eq!(params.layer, layer);
        stream.wait_for(unsafe { &params.sync_event.sprout(stream.ctx()) });

        params
    }

    pub unsafe fn kill(&mut self, ctx: &ContextGuard) {
        for layer in &mut self.layers {
            layer.input_layernorm.physical_mut().kill(ctx);
            layer.w_qkv.physical_mut().kill(ctx);
            layer.self_attn_o_proj.physical_mut().kill(ctx);
            layer.post_attention_layernorm.physical_mut().kill(ctx);
            layer.mlp_gate_up.physical_mut().kill(ctx);
            layer.mlp_down.physical_mut().kill(ctx);
            layer.sync_event.kill(ctx);
        }
    }
}

pub(crate) struct LayerParameter {
    pub input_layernorm: Tensor<DevMemSpore>,
    pub w_qkv: Tensor<DevMemSpore>,
    pub self_attn_o_proj: Tensor<DevMemSpore>,
    pub post_attention_layernorm: Tensor<DevMemSpore>,
    pub mlp_gate_up: Tensor<DevMemSpore>,
    pub mlp_down: Tensor<DevMemSpore>,

    layer: usize,
    sync_event: EventSpore,
}

impl LayerParameter {
    #[inline]
    pub fn input_layernorm<'ctx>(&self, ctx: &'ctx ContextGuard) -> Tensor<DevMem<'ctx>> {
        unsafe {
            self.input_layernorm
                .as_ref()
                .map_physical(|s| s.sprout(ctx))
        }
    }

    #[inline]
    pub fn w_qkv<'ctx>(&self, ctx: &'ctx ContextGuard) -> Tensor<DevMem<'ctx>> {
        unsafe { self.w_qkv.as_ref().map_physical(|s| s.sprout(ctx)) }
    }

    #[inline]
    pub fn w_o<'ctx>(&self, ctx: &'ctx ContextGuard) -> Tensor<DevMem<'ctx>> {
        unsafe {
            self.self_attn_o_proj
                .as_ref()
                .map_physical(|s| s.sprout(ctx))
        }
    }

    #[inline]
    pub fn post_attention_layernorm<'ctx>(&self, ctx: &'ctx ContextGuard) -> Tensor<DevMem<'ctx>> {
        unsafe {
            self.post_attention_layernorm
                .as_ref()
                .map_physical(|s| s.sprout(ctx))
        }
    }

    #[inline]
    pub fn mlp_gate_up<'ctx>(&self, ctx: &'ctx ContextGuard) -> Tensor<DevMem<'ctx>> {
        unsafe { self.mlp_gate_up.as_ref().map_physical(|s| s.sprout(ctx)) }
    }

    #[inline]
    pub fn mlp_down<'ctx>(&self, ctx: &'ctx ContextGuard) -> Tensor<DevMem<'ctx>> {
        unsafe { self.mlp_down.as_ref().map_physical(|s| s.sprout(ctx)) }
    }

    fn new(host: &dyn Llama2, layer: usize, stream: &Stream) -> Self {
        macro_rules! map {
            ($param:ident) => {
                host.$param(layer)
                    .as_ref()
                    .map_physical(|slice| stream.from_host(slice).sporulate())
            };
        }
        Self {
            input_layernorm: map!(input_layernorm),
            w_qkv: map!(w_qkv).transpose(&[1, 0]),
            self_attn_o_proj: map!(self_attn_o_proj).transpose(&[1, 0]),
            post_attention_layernorm: map!(post_attention_layernorm),
            mlp_gate_up: map!(mlp_gate_up).transpose(&[1, 0]),
            mlp_down: map!(mlp_down).transpose(&[1, 0]),
            layer,
            sync_event: stream.record().sporulate(),
        }
    }

    fn load(&mut self, host: &dyn Llama2, layer: usize, stream: &Stream) {
        if self.layer == layer {
            return;
        }

        let ctx = stream.ctx();
        macro_rules! update {
            ($param:ident) => {
                stream.memcpy_h2d(
                    unsafe { &mut self.$param.physical_mut().sprout(ctx) },
                    host.$param(layer).as_slice(),
                )
            };
        }
        update!(input_layernorm);
        update!(w_qkv);
        update!(self_attn_o_proj);
        update!(post_attention_layernorm);
        update!(mlp_gate_up);
        update!(mlp_down);

        unsafe { self.sync_event.kill(stream.ctx()) };
        self.sync_event = stream.record().sporulate();
        self.layer = layer;
    }
}
