mod cast;
mod memory;
mod safe_tensors;
mod save;
mod storage;

use common::utok;
use tensor::{DataType, Tensor};
mod distribute;

pub use distribute::{DistributeScheme, DistributedLayer, Distributer};
pub use memory::Memory;
pub use save::save;
pub use storage::Storage;

pub trait Llama2 {
    fn bos_token_id(&self) -> utok;
    fn eos_token_id(&self) -> utok;
    fn hidden_size(&self) -> usize;
    fn intermediate_size(&self) -> usize;
    fn max_position_embeddings(&self) -> usize;
    fn num_attention_heads(&self) -> usize;
    fn num_hidden_layers(&self) -> usize;
    fn num_key_value_heads(&self) -> usize;
    fn vocab_size(&self) -> usize;
    fn rms_norm_eps(&self) -> f32;
    fn rope_theta(&self) -> f32;
    fn data_type(&self) -> DataType;

    #[inline]
    fn kv_hidden_size(&self) -> usize {
        self.hidden_size() * self.num_key_value_heads() / self.num_attention_heads()
    }

    fn size(&self) -> usize {
        let d = self.hidden_size();
        let dv = self.vocab_size();
        let dkv = self.kv_hidden_size();
        let di = self.intermediate_size();
        let l = self.num_hidden_layers();

        (d * dv      // embed_tokens
       + l * d       // input_layernorm
       + l * d * d   // self_attn_q_proj
       + l * dkv * d // self_attn_k_proj
       + l * dkv * d // self_attn_v_proj
       + l * d * d   // self_attn_o_proj
       + l * d       // post_attention_layernorm
       + l * di * d  // mlp_gate
       + l * d * di  // mlp_down
       + l * di * d  // mlp_up
       + d           // model_norm
       + dv * d)     // lm_head
       * self.data_type().size()
    }

    /// Shape = `vocab_size x hidden_size`.
    fn embed_tokens(&self) -> Tensor<Storage>;
    /// Shape = `hidden_size`.
    fn input_layernorm(&self, layer: usize) -> Tensor<Storage>;
    /// Shape = `(((num_head + num_kv_head + num_kv_head) x head_dim) x hidden_size`.
    fn w_qkv(&self, layer: usize) -> Tensor<Storage>;
    /// Shape = `(num_kv_head x head_group x head_dim) x hidden_size`.
    fn self_attn_q_proj(&self, layer: usize) -> Tensor<Storage>;
    /// Shape = `(num_kv_head x head_dim) x hidden_size`.
    fn self_attn_k_proj(&self, layer: usize) -> Tensor<Storage>;
    /// Shape = `(num_kv_head x head_dim) x hidden_size`.
    fn self_attn_v_proj(&self, layer: usize) -> Tensor<Storage>;
    /// Shape = `(num_kv_head x head_group x head_dim) x hidden_size`.
    fn self_attn_o_proj(&self, layer: usize) -> Tensor<Storage>;
    /// Shape = `hidden_size`.
    fn post_attention_layernorm(&self, layer: usize) -> Tensor<Storage>;
    /// Shape = `(intermediate_size + intermediate_size) x hidden_size`.
    fn mlp_gate_up(&self, layer: usize) -> Tensor<Storage>;
    /// Shape = `intermediate_size x hidden_size`.
    fn mlp_gate(&self, layer: usize) -> Tensor<Storage>;
    /// Shape = `intermediate_size x hidden_size`.
    fn mlp_up(&self, layer: usize) -> Tensor<Storage>;
    /// Shape = `hidden_size x intermediate_size`.
    fn mlp_down(&self, layer: usize) -> Tensor<Storage>;
    /// Shape = `hidden_size`.
    fn model_norm(&self) -> Tensor<Storage>;
    /// Shape = `vocab_size x hidden_size`.
    fn lm_head(&self) -> Tensor<Storage>;

    fn tensors(&self) -> Vec<Tensor<Storage>> {
        let mut tensors = Vec::with_capacity(self.num_hidden_layers() * 6 + 3);
        tensors.push(self.embed_tokens());
        tensors.push(self.embed_tokens());
        for layer in 0..self.num_hidden_layers() {
            tensors.push(self.input_layernorm(layer));
            tensors.push(self.w_qkv(layer));
            tensors.push(self.self_attn_o_proj(layer));
            tensors.push(self.post_attention_layernorm(layer));
            tensors.push(self.mlp_gate_up(layer));
            tensors.push(self.mlp_down(layer));
        }
        tensors.push(self.model_norm());
        tensors.push(self.lm_head());
        tensors
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
struct ConfigJson {
    pub bos_token_id: utok,
    pub eos_token_id: utok,
    pub hidden_size: usize,
    pub intermediate_size: usize,
    pub max_position_embeddings: usize,
    pub num_attention_heads: usize,
    pub num_hidden_layers: usize,
    pub num_key_value_heads: usize,
    pub vocab_size: usize,
    #[serde(default = "default_rms_norm_eps")]
    pub rms_norm_eps: f32,
    #[serde(default = "default_rope_theta")]
    pub rope_theta: f32,
    pub torch_dtype: DataType,
}

#[inline(always)]
const fn default_rms_norm_eps() -> f32 {
    1e-5
}

#[inline(always)]
const fn default_rope_theta() -> f32 {
    1e4
}

impl From<&dyn Llama2> for ConfigJson {
    fn from(model: &dyn Llama2) -> Self {
        Self {
            bos_token_id: model.bos_token_id(),
            eos_token_id: model.eos_token_id(),
            hidden_size: model.hidden_size(),
            intermediate_size: model.intermediate_size(),
            max_position_embeddings: model.max_position_embeddings(),
            num_attention_heads: model.num_attention_heads(),
            num_hidden_layers: model.num_hidden_layers(),
            num_key_value_heads: model.num_key_value_heads(),
            vocab_size: model.vocab_size(),
            rms_norm_eps: model.rms_norm_eps(),
            rope_theta: model.rope_theta(),
            torch_dtype: model.data_type(),
        }
    }
}
