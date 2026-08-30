use std::collections::{BTreeSet, HashMap, VecDeque};

use thiserror::Error;

use crate::scpt::{
    Scpt, ScptBinaryOperator, ScptInstructionKind, ScptUnaryOperator, ScptValueKind,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlFlowEdgeKind {
    Fallthrough,
    Jump,
    True,
    False,
    Equal,
    Unequal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlFlowEdge {
    pub kind: ControlFlowEdgeKind,
    pub target: usize,
}

#[derive(Debug, Eq, PartialEq)]
pub struct BasicBlock {
    pub first_instruction: usize,
    pub end_instruction: usize,
    pub successors: Vec<ControlFlowEdge>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct ControlFlowGraph {
    pub blocks: Vec<BasicBlock>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StackValueType {
    Integer,
    Float,
    String,
    Unknown,
}

#[derive(Debug, Error)]
pub enum StackFlowError {
    #[error("SCPT instruction {instruction} pops an empty value stack")]
    Underflow { instruction: usize },
    #[error("SCPT block {block} receives incompatible stacks {first:?} and {second:?}")]
    Merge {
        block: usize,
        first: Vec<StackValueType>,
        second: Vec<StackValueType>,
    },
    #[error("SCPT instruction {instruction} calls unit {unit} with stack {stack:?}")]
    UnitArguments {
        instruction: usize,
        unit: u16,
        stack: Vec<StackValueType>,
    },
}

impl ControlFlowGraph {
    pub fn build(script: &Scpt) -> Self {
        let instruction_count = script.instructions.len();
        let mut starts = BTreeSet::from([0]);
        for unit in &script.units {
            if let Ok(start) = usize::try_from(unit.first_instruction) {
                starts.insert(start);
            }
        }
        for (index, instruction) in script.instructions.iter().enumerate() {
            let has_sequential_successor = match instruction.kind() {
                ScptInstructionKind::Jump { target } => {
                    insert_target(&mut starts, target);
                    false
                }
                ScptInstructionKind::Branch {
                    true_target,
                    false_target,
                } => {
                    insert_target(&mut starts, true_target);
                    insert_target(&mut starts, false_target);
                    false
                }
                ScptInstructionKind::EqualBranch {
                    equal_target,
                    unequal_target,
                } => {
                    insert_target(&mut starts, equal_target);
                    insert_target(&mut starts, unequal_target);
                    false
                }
                ScptInstructionKind::Return | ScptInstructionKind::ReturnValue => false,
                _ => true,
            };
            if !has_sequential_successor && index + 1 < instruction_count {
                starts.insert(index + 1);
            }
        }

        let starts = starts.into_iter().collect::<Vec<_>>();
        let blocks = starts
            .iter()
            .enumerate()
            .map(|(block, first)| {
                let end = starts.get(block + 1).copied().unwrap_or(instruction_count);
                BasicBlock {
                    first_instruction: *first,
                    end_instruction: end,
                    successors: successors(script, end, instruction_count),
                }
            })
            .collect();
        Self { blocks }
    }

    pub fn validate_stack_types(&self, script: &Scpt) -> Result<(), StackFlowError> {
        let block_by_start = self
            .blocks
            .iter()
            .enumerate()
            .map(|(block, value)| (value.first_instruction, block))
            .collect::<HashMap<_, _>>();
        let mut input_stacks = vec![None; self.blocks.len()];
        let mut work = VecDeque::new();
        let mut roots = BTreeSet::from([0]);
        for unit in &script.units {
            if let Ok(start) = usize::try_from(unit.first_instruction) {
                roots.insert(start);
            }
        }
        for root in roots {
            if let Some(block) = block_by_start.get(&root).copied() {
                set_input_stack(&mut input_stacks, &mut work, block, Vec::new())?;
            }
        }

        while let Some(block_index) = work.pop_front() {
            let block = &self.blocks[block_index];
            let mut stack = input_stacks[block_index].clone().unwrap_or_default();
            for instruction_index in block.first_instruction..block.end_instruction {
                apply_instruction(script, instruction_index, &mut stack)?;
            }
            for edge in &block.successors {
                if let Some(target_block) = block_by_start.get(&edge.target).copied() {
                    set_input_stack(&mut input_stacks, &mut work, target_block, stack.clone())?;
                }
            }
        }
        Ok(())
    }
}

fn successors(script: &Scpt, end: usize, instruction_count: usize) -> Vec<ControlFlowEdge> {
    let Some(instruction) = end
        .checked_sub(1)
        .and_then(|index| script.instructions.get(index))
    else {
        return Vec::new();
    };
    match instruction.kind() {
        ScptInstructionKind::Jump { target } => edge(ControlFlowEdgeKind::Jump, target),
        ScptInstructionKind::Branch {
            true_target,
            false_target,
        } => [
            edge(ControlFlowEdgeKind::True, true_target),
            edge(ControlFlowEdgeKind::False, false_target),
        ]
        .concat(),
        ScptInstructionKind::EqualBranch {
            equal_target,
            unequal_target,
        } => [
            edge(ControlFlowEdgeKind::Equal, equal_target),
            edge(ControlFlowEdgeKind::Unequal, unequal_target),
        ]
        .concat(),
        ScptInstructionKind::Return | ScptInstructionKind::ReturnValue => Vec::new(),
        _ if end < instruction_count => vec![ControlFlowEdge {
            kind: ControlFlowEdgeKind::Fallthrough,
            target: end,
        }],
        _ => Vec::new(),
    }
}

fn edge(kind: ControlFlowEdgeKind, target: u32) -> Vec<ControlFlowEdge> {
    usize::try_from(target)
        .ok()
        .map_or_else(Vec::new, |target| vec![ControlFlowEdge { kind, target }])
}

fn set_input_stack(
    inputs: &mut [Option<Vec<StackValueType>>],
    work: &mut VecDeque<usize>,
    block: usize,
    stack: Vec<StackValueType>,
) -> Result<(), StackFlowError> {
    match &inputs[block] {
        Some(existing) if *existing != stack => Err(StackFlowError::Merge {
            block,
            first: existing.clone(),
            second: stack,
        }),
        Some(_) => Ok(()),
        None => {
            inputs[block] = Some(stack);
            work.push_back(block);
            Ok(())
        }
    }
}

fn apply_instruction(
    script: &Scpt,
    instruction_index: usize,
    stack: &mut Vec<StackValueType>,
) -> Result<(), StackFlowError> {
    let instruction = &script.instructions[instruction_index];
    match instruction.kind() {
        ScptInstructionKind::NoOperation
        | ScptInstructionKind::Reserved
        | ScptInstructionKind::Jump { .. }
        | ScptInstructionKind::Return => {}
        ScptInstructionKind::PushValue {
            value,
            value_type,
            indexed,
        } => {
            if indexed {
                pop(stack, instruction_index)?;
            }
            stack.push(match u8::try_from(value_type).ok() {
                Some(b'A') => stored_value_type(script, value),
                Some(b'F') => StackValueType::Float,
                Some(b'I') => StackValueType::Integer,
                _ => StackValueType::Unknown,
            });
        }
        ScptInstructionKind::StoreValue { indexed, .. }
        | ScptInstructionKind::AssignmentOperator { indexed, .. } => {
            pop(stack, instruction_index)?;
            if indexed {
                pop(stack, instruction_index)?;
            }
        }
        ScptInstructionKind::BinaryOperator(operator) => {
            let right = pop(stack, instruction_index)?;
            let left = pop(stack, instruction_index)?;
            stack.push(binary_type(operator, left, right));
        }
        ScptInstructionKind::UnaryOperator(operator) => {
            let value = pop(stack, instruction_index)?;
            stack.push(match operator {
                ScptUnaryOperator::Negate => value,
                ScptUnaryOperator::LogicalNot | ScptUnaryOperator::BitwiseNot => {
                    StackValueType::Integer
                }
            });
        }
        ScptInstructionKind::Increment { indexed, .. }
        | ScptInstructionKind::Decrement { indexed, .. } => {
            if indexed {
                pop(stack, instruction_index)?;
            }
        }
        ScptInstructionKind::Branch { .. } => {
            pop(stack, instruction_index)?;
        }
        ScptInstructionKind::EqualBranch { .. } => {
            pop(stack, instruction_index)?;
            pop(stack, instruction_index)?;
        }
        ScptInstructionKind::ReturnValue => {
            pop(stack, instruction_index)?;
        }
        ScptInstructionKind::NativeCall { argument_count, .. } => {
            for _ in 0..argument_count {
                pop(stack, instruction_index)?;
            }
        }
        ScptInstructionKind::UnitCall { unit } => {
            if !stack.is_empty() {
                return Err(StackFlowError::UnitArguments {
                    instruction: instruction_index,
                    unit,
                    stack: stack.clone(),
                });
            }
        }
    }
    Ok(())
}

fn pop(
    stack: &mut Vec<StackValueType>,
    instruction: usize,
) -> Result<StackValueType, StackFlowError> {
    stack.pop().ok_or(StackFlowError::Underflow { instruction })
}

fn stored_value_type(script: &Scpt, value: u32) -> StackValueType {
    usize::try_from(value)
        .ok()
        .and_then(|index| script.values.get(index))
        .map_or(StackValueType::Unknown, |value| match value.kind {
            ScptValueKind::Integer => StackValueType::Integer,
            ScptValueKind::Float => StackValueType::Float,
            ScptValueKind::String => StackValueType::String,
        })
}

fn binary_type(
    operator: ScptBinaryOperator,
    left: StackValueType,
    right: StackValueType,
) -> StackValueType {
    match operator {
        ScptBinaryOperator::LessThan
        | ScptBinaryOperator::LessThanOrEqual
        | ScptBinaryOperator::GreaterThan
        | ScptBinaryOperator::GreaterThanOrEqual
        | ScptBinaryOperator::Equal
        | ScptBinaryOperator::NotEqual
        | ScptBinaryOperator::LogicalAnd
        | ScptBinaryOperator::LogicalOr
        | ScptBinaryOperator::BitwiseAnd
        | ScptBinaryOperator::BitwiseOr
        | ScptBinaryOperator::BitwiseXor
        | ScptBinaryOperator::ShiftLeft
        | ScptBinaryOperator::ShiftRight
        | ScptBinaryOperator::Modulo => StackValueType::Integer,
        ScptBinaryOperator::Add
        | ScptBinaryOperator::Subtract
        | ScptBinaryOperator::Multiply
        | ScptBinaryOperator::Divide => {
            if left == StackValueType::Float || right == StackValueType::Float {
                StackValueType::Float
            } else if left == StackValueType::Unknown || right == StackValueType::Unknown {
                StackValueType::Unknown
            } else {
                StackValueType::Integer
            }
        }
    }
}

fn insert_target(starts: &mut BTreeSet<usize>, target: u32) {
    if let Ok(target) = usize::try_from(target) {
        starts.insert(target);
    }
}
