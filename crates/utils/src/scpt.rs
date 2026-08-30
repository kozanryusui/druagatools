use std::io::Cursor;

use binrw::{BinRead, binread};
use thiserror::Error;

const HEADER_SIZE: u32 = 0x20;
const VALUE_RECORD_SIZE: u32 = 6;
const UNIT_RECORD_SIZE: u32 = 9;
const INSTRUCTION_RECORD_SIZE: u32 = 12;

#[binread]
#[derive(Debug)]
#[br(little, magic = b"scpt")]
pub struct Scpt {
    #[br(assert(header_size == HEADER_SIZE))]
    pub header_size: u32,
    pub constant_08: u16,
    pub native_function_count: u16,
    #[br(assert(value_section_size.is_multiple_of(VALUE_RECORD_SIZE)))]
    pub value_section_size: u32,
    #[br(assert(unit_section_size.is_multiple_of(UNIT_RECORD_SIZE)))]
    pub unit_section_size: u32,
    pub string_section_size: u32,
    #[br(assert(instruction_section_size.is_multiple_of(INSTRUCTION_RECORD_SIZE)))]
    pub instruction_section_size: u32,
    pub script_name: [u8; 4],
    #[br(count = value_section_size / VALUE_RECORD_SIZE)]
    pub values: Vec<ScptValue>,
    #[br(count = unit_section_size / UNIT_RECORD_SIZE)]
    pub units: Vec<ScptUnit>,
    #[br(count = string_section_size)]
    pub strings: Vec<u8>,
    #[br(count = instruction_section_size / INSTRUCTION_RECORD_SIZE)]
    pub instructions: Vec<ScptInstruction>,
}

#[derive(BinRead, Clone, Copy, Debug, Eq, PartialEq)]
#[br(repr = u8)]
#[repr(u8)]
pub enum ScptValueKind {
    Integer = b'I',
    Float = b'F',
    String = b'S',
}

#[derive(BinRead, Clone, Copy, Debug, Eq, PartialEq)]
#[br(little)]
pub struct ScptValue {
    pub kind: ScptValueKind,
    pub attribute: u8,
    pub raw_value: u32,
}

#[derive(BinRead, Clone, Copy, Debug, Eq, PartialEq)]
#[br(repr = u8)]
#[repr(u8)]
pub enum ScptUnitKind {
    InclusiveEnd = b'U',
    ExclusiveEnd = b'I',
}

#[derive(BinRead, Clone, Copy, Debug, Eq, PartialEq)]
#[br(little)]
pub struct ScptUnit {
    pub kind: ScptUnitKind,
    pub first_instruction: u32,
    pub end_instruction: u32,
}

#[derive(BinRead, Clone, Copy, Debug, Eq, PartialEq)]
#[br(little)]
pub struct ScptInstruction {
    pub opcode: u32,
    pub operand_1: u32,
    pub operand_2: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScptInstructionKind {
    NoOperation,
    PushValue {
        value: u32,
        value_type: u16,
        indexed: bool,
    },
    Reserved,
    Jump {
        target: u32,
    },
    StoreValue {
        value: u32,
        indexed: bool,
    },
    BinaryOperator(ScptBinaryOperator),
    UnaryOperator(ScptUnaryOperator),
    AssignmentOperator {
        value: u32,
        indexed: bool,
        operator: ScptAssignmentOperator,
    },
    Increment {
        value: u32,
        indexed: bool,
    },
    Decrement {
        value: u32,
        indexed: bool,
    },
    Branch {
        true_target: u32,
        false_target: u32,
    },
    EqualBranch {
        equal_target: u32,
        unequal_target: u32,
    },
    Return,
    ReturnValue,
    NativeCall {
        function: u16,
        argument_count: u16,
        result_value: Option<u32>,
    },
    UnitCall {
        unit: u16,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScptBinaryOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
    Equal,
    NotEqual,
    LogicalAnd,
    LogicalOr,
    BitwiseAnd,
    BitwiseOr,
    BitwiseXor,
    ShiftLeft,
    ShiftRight,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScptUnaryOperator {
    Negate,
    LogicalNot,
    BitwiseNot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScptAssignmentOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    BitwiseAnd,
    BitwiseOr,
    BitwiseXor,
    ShiftLeft,
    ShiftRight,
}

#[derive(Debug, Error)]
pub enum ScptError {
    #[error("SCPT parse failed: {0}")]
    Parse(#[from] binrw::Error),
    #[error("SCPT has {actual} bytes, but its sections account for {expected} bytes")]
    FileSize { actual: usize, expected: u64 },
    #[error("SCPT string section does not end with a null byte")]
    UnterminatedStringSection,
    #[error(
        "SCPT unit {unit} ({kind:?}) has instruction bounds {first} and {end}, but the file has {instruction_count} instructions"
    )]
    UnitRange {
        unit: usize,
        kind: ScptUnitKind,
        first: u32,
        end: u32,
        instruction_count: usize,
    },
    #[error(
        "SCPT instruction {instruction} opcode {opcode} targets instruction {target}, but the file has {instruction_count} instructions"
    )]
    InstructionTarget {
        instruction: usize,
        opcode: u32,
        target: u32,
        instruction_count: usize,
    },
    #[error(
        "SCPT instruction {instruction} calls unit {unit}, but the file has {unit_count} units"
    )]
    UnitTarget {
        instruction: usize,
        unit: u16,
        unit_count: usize,
    },
    #[error(
        "SCPT instruction {instruction} calls native function {function}, but the compile-time table has {native_function_count} entries"
    )]
    NativeFunction {
        instruction: usize,
        function: u16,
        native_function_count: u16,
    },
    #[error("SCPT instruction {instruction} uses invalid opcode {opcode}")]
    InvalidOpcode { instruction: usize, opcode: u32 },
}

impl Scpt {
    pub fn parse(bytes: &[u8]) -> Result<Self, ScptError> {
        let mut cursor = Cursor::new(bytes);
        let result = Self::read_le(&mut cursor)?;
        let expected = u64::from(result.header_size)
            + u64::from(result.value_section_size)
            + u64::from(result.unit_section_size)
            + u64::from(result.string_section_size)
            + u64::from(result.instruction_section_size);
        if expected != bytes.len() as u64 {
            return Err(ScptError::FileSize {
                actual: bytes.len(),
                expected,
            });
        }
        if result.strings.last().is_some_and(|value| *value != 0) {
            return Err(ScptError::UnterminatedStringSection);
        }

        let instruction_count = result.instructions.len();
        for (unit, range) in result.units.iter().enumerate() {
            let valid = range.first_instruction <= range.end_instruction
                && usize::try_from(range.end_instruction).is_ok_and(|end| match range.kind {
                    ScptUnitKind::InclusiveEnd => end < instruction_count,
                    ScptUnitKind::ExclusiveEnd => end <= instruction_count,
                });
            if !valid {
                return Err(ScptError::UnitRange {
                    unit,
                    kind: range.kind,
                    first: range.first_instruction,
                    end: range.end_instruction,
                    instruction_count,
                });
            }
        }
        result.validate_instruction_references()?;
        Ok(result)
    }

    pub fn string_count(&self) -> usize {
        self.strings.iter().filter(|value| **value == 0).count()
    }

    pub fn value_section_offset(&self) -> u64 {
        u64::from(self.header_size)
    }

    pub fn unit_section_offset(&self) -> u64 {
        self.value_section_offset() + u64::from(self.value_section_size)
    }

    pub fn string_section_offset(&self) -> u64 {
        self.unit_section_offset() + u64::from(self.unit_section_size)
    }

    pub fn instruction_section_offset(&self) -> u64 {
        self.string_section_offset() + u64::from(self.string_section_size)
    }

    pub fn string_at(&self, offset: u32) -> Option<&[u8]> {
        let start = usize::try_from(offset).ok()?;
        let remaining = self.strings.get(start..)?;
        let length = remaining.iter().position(|value| *value == 0)?;
        Some(&remaining[..length])
    }

    fn validate_instruction_references(&self) -> Result<(), ScptError> {
        for (index, instruction) in self.instructions.iter().enumerate() {
            if instruction.opcode == 2 || instruction.opcode >= 49 {
                return Err(ScptError::InvalidOpcode {
                    instruction: index,
                    opcode: instruction.opcode,
                });
            }
            match instruction.kind() {
                ScptInstructionKind::Jump { target } => {
                    self.validate_instruction_target(index, instruction.opcode, target)?;
                }
                ScptInstructionKind::Branch {
                    true_target,
                    false_target,
                } => {
                    self.validate_instruction_target(index, instruction.opcode, true_target)?;
                    self.validate_instruction_target(index, instruction.opcode, false_target)?;
                }
                ScptInstructionKind::EqualBranch {
                    equal_target,
                    unequal_target,
                } => {
                    self.validate_instruction_target(index, instruction.opcode, equal_target)?;
                    self.validate_instruction_target(index, instruction.opcode, unequal_target)?;
                }
                ScptInstructionKind::NativeCall { function, .. } => {
                    if function >= self.native_function_count {
                        return Err(ScptError::NativeFunction {
                            instruction: index,
                            function,
                            native_function_count: self.native_function_count,
                        });
                    }
                }
                ScptInstructionKind::UnitCall { unit } => {
                    if usize::from(unit) >= self.units.len() {
                        return Err(ScptError::UnitTarget {
                            instruction: index,
                            unit,
                            unit_count: self.units.len(),
                        });
                    }
                }
                ScptInstructionKind::NoOperation
                | ScptInstructionKind::PushValue { .. }
                | ScptInstructionKind::Reserved
                | ScptInstructionKind::StoreValue { .. }
                | ScptInstructionKind::BinaryOperator(_)
                | ScptInstructionKind::UnaryOperator(_)
                | ScptInstructionKind::AssignmentOperator { .. }
                | ScptInstructionKind::Increment { .. }
                | ScptInstructionKind::Decrement { .. }
                | ScptInstructionKind::Return
                | ScptInstructionKind::ReturnValue => {}
            }
        }
        Ok(())
    }

    fn validate_instruction_target(
        &self,
        instruction: usize,
        opcode: u32,
        target: u32,
    ) -> Result<(), ScptError> {
        if usize::try_from(target).is_ok_and(|value| value <= self.instructions.len()) {
            Ok(())
        } else {
            Err(ScptError::InstructionTarget {
                instruction,
                opcode,
                target,
                instruction_count: self.instructions.len(),
            })
        }
    }
}

impl ScptInstruction {
    pub fn kind(&self) -> ScptInstructionKind {
        match self.opcode {
            0 | 41 | 42 | 44 => ScptInstructionKind::NoOperation,
            1 => ScptInstructionKind::PushValue {
                value: self.operand_1,
                value_type: (self.operand_2 >> 16) as u16,
                indexed: self.operand_2 as u16 != 0,
            },
            2 => ScptInstructionKind::Reserved,
            3 => ScptInstructionKind::Jump {
                target: self.operand_1,
            },
            4 => ScptInstructionKind::StoreValue {
                value: self.operand_1,
                indexed: self.operand_2 != 0,
            },
            5 => ScptInstructionKind::BinaryOperator(ScptBinaryOperator::Add),
            6 => ScptInstructionKind::BinaryOperator(ScptBinaryOperator::Subtract),
            7 => ScptInstructionKind::BinaryOperator(ScptBinaryOperator::Multiply),
            8 => ScptInstructionKind::BinaryOperator(ScptBinaryOperator::Divide),
            9 => ScptInstructionKind::BinaryOperator(ScptBinaryOperator::Modulo),
            10 => ScptInstructionKind::UnaryOperator(ScptUnaryOperator::Negate),
            11 => self.assignment(ScptAssignmentOperator::Add),
            12 => self.assignment(ScptAssignmentOperator::Subtract),
            13 => self.assignment(ScptAssignmentOperator::Multiply),
            14 => self.assignment(ScptAssignmentOperator::Divide),
            15 => self.assignment(ScptAssignmentOperator::Modulo),
            16 => ScptInstructionKind::Increment {
                value: self.operand_1,
                indexed: self.operand_2 != 0,
            },
            17 => ScptInstructionKind::Decrement {
                value: self.operand_1,
                indexed: self.operand_2 != 0,
            },
            18 => ScptInstructionKind::BinaryOperator(ScptBinaryOperator::GreaterThan),
            19 => ScptInstructionKind::BinaryOperator(ScptBinaryOperator::GreaterThanOrEqual),
            20 => ScptInstructionKind::BinaryOperator(ScptBinaryOperator::LessThan),
            21 => ScptInstructionKind::BinaryOperator(ScptBinaryOperator::LessThanOrEqual),
            22 => ScptInstructionKind::BinaryOperator(ScptBinaryOperator::Equal),
            23 => ScptInstructionKind::BinaryOperator(ScptBinaryOperator::NotEqual),
            24 => ScptInstructionKind::BinaryOperator(ScptBinaryOperator::LogicalAnd),
            25 => ScptInstructionKind::BinaryOperator(ScptBinaryOperator::LogicalOr),
            26 => ScptInstructionKind::UnaryOperator(ScptUnaryOperator::LogicalNot),
            27 => ScptInstructionKind::BinaryOperator(ScptBinaryOperator::BitwiseAnd),
            28 => ScptInstructionKind::BinaryOperator(ScptBinaryOperator::BitwiseOr),
            29 => ScptInstructionKind::BinaryOperator(ScptBinaryOperator::BitwiseXor),
            30 => ScptInstructionKind::UnaryOperator(ScptUnaryOperator::BitwiseNot),
            31 => ScptInstructionKind::BinaryOperator(ScptBinaryOperator::ShiftLeft),
            32 => ScptInstructionKind::BinaryOperator(ScptBinaryOperator::ShiftRight),
            33 => self.assignment(ScptAssignmentOperator::BitwiseAnd),
            34 => self.assignment(ScptAssignmentOperator::BitwiseOr),
            35 => self.assignment(ScptAssignmentOperator::BitwiseXor),
            36 => self.assignment(ScptAssignmentOperator::ShiftLeft),
            37 => self.assignment(ScptAssignmentOperator::ShiftRight),
            38..=40 => ScptInstructionKind::Branch {
                true_target: self.operand_1,
                false_target: self.operand_2,
            },
            43 => ScptInstructionKind::EqualBranch {
                equal_target: self.operand_1,
                unequal_target: self.operand_2,
            },
            45 => ScptInstructionKind::Return,
            46 => ScptInstructionKind::ReturnValue,
            47 => ScptInstructionKind::NativeCall {
                function: (self.operand_2 >> 16) as u16,
                argument_count: self.operand_2 as u16,
                result_value: (self.operand_1 != u32::MAX).then_some(self.operand_1),
            },
            48 => ScptInstructionKind::UnitCall {
                unit: (self.operand_2 >> 16) as u16,
            },
            _ => ScptInstructionKind::Reserved,
        }
    }

    fn assignment(&self, operator: ScptAssignmentOperator) -> ScptInstructionKind {
        ScptInstructionKind::AssignmentOperator {
            value: self.operand_1,
            indexed: self.operand_2 != 0,
            operator,
        }
    }
}
