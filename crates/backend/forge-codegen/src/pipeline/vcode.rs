//! VCode — 虚拟寄存器代码表示。
//!
//! 将 IR 指令 lowering 为机器指令后，指令操作数使用 VReg（虚拟寄存器）。
//! VCode 是寄存器分配的输入。

use forge_ir::Block;

/// VCode 基本块标识。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VBlockId(pub u32);

/// VCode 基本块。
#[derive(Debug, Clone)]
pub struct VCodeBlock<I> {
    pub id: VBlockId,
    /// 对应的 IR Block。
    pub ir_block: Block,
    /// 块内指令序列。
    pub instructions: Vec<I>,
    /// 是否为返回块（terminator 是 Return）。
    pub is_return_block: bool,
}

impl<I> VCodeBlock<I> {
    pub fn new(id: VBlockId, ir_block: Block) -> Self {
        Self {
            id,
            ir_block,
            instructions: Vec::new(),
            is_return_block: false,
        }
    }
}

/// VCode — 由机器指令组成、操作数为虚拟寄存器的代码表示。
pub struct VCode<I> {
    blocks: Vec<VCodeBlock<I>>,
    current_block: Option<VBlockId>,
}

impl<I> Default for VCode<I> {
    fn default() -> Self {
        Self::new()
    }
}

impl<I> VCode<I> {
    pub fn new() -> Self {
        Self {
            blocks: Vec::new(),
            current_block: None,
        }
    }

    /// 创建一个新块，绑定到指定的 IR Block。
    pub fn create_block(&mut self, ir_block: Block) -> VBlockId {
        let id = VBlockId(self.blocks.len() as u32);
        self.blocks.push(VCodeBlock::new(id, ir_block));
        id
    }

    /// 切换到指定块，后续 `push_inst` 将向该块添加指令。
    pub fn switch_to_block(&mut self, block: VBlockId) {
        assert!(
            (block.0 as usize) < self.blocks.len(),
            "VCode block {:?} does not exist",
            block
        );
        self.current_block = Some(block);
    }

    /// 向当前块添加一条机器指令。
    pub fn push_inst(&mut self, inst: I) {
        let block_id = self.current_block.expect("no current VCode block");
        self.blocks[block_id.0 as usize].instructions.push(inst);
    }

    /// 迭代所有块。
    pub fn blocks(&self) -> impl Iterator<Item = &VCodeBlock<I>> {
        self.blocks.iter()
    }

    /// 可变迭代所有块。
    pub fn blocks_mut(&mut self) -> impl Iterator<Item = &mut VCodeBlock<I>> {
        self.blocks.iter_mut()
    }

    /// 按 ID 获取块。
    pub fn block(&self, id: VBlockId) -> Option<&VCodeBlock<I>> {
        self.blocks.get(id.0 as usize)
    }

    /// 按 ID 获取可变块引用。
    pub fn block_mut(&mut self, id: VBlockId) -> Option<&mut VCodeBlock<I>> {
        self.blocks.get_mut(id.0 as usize)
    }

    /// 获取块数量。
    pub fn num_blocks(&self) -> usize {
        self.blocks.len()
    }

    /// 获取所有块中的总指令数。
    pub fn num_insts(&self) -> usize {
        self.blocks.iter().map(|b| b.instructions.len()).sum()
    }

    /// 获取 IR Block → VCodeBlock 的映射。
    pub fn ir_to_vcode_map(&self) -> Vec<(Block, VBlockId)> {
        self.blocks.iter().map(|b| (b.ir_block, b.id)).collect()
    }
}

impl<I: Clone> Clone for VCode<I> {
    fn clone(&self) -> Self {
        Self {
            blocks: self.blocks.clone(),
            current_block: self.current_block,
        }
    }
}
