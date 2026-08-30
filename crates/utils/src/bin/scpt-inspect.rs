use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use druaga_utils::scpt::{
    Scpt, ScptAssignmentOperator, ScptBinaryOperator, ScptError, ScptInstruction,
    ScptInstructionKind, ScptUnaryOperator, ScptUnitKind, ScptValueKind,
};
use druaga_utils::scpt_control_flow::{ControlFlowGraph, StackFlowError};
use druaga_utils::scpt_decompile;
use druaga_utils::scpt_decompile::DecompileOptions;
use druaga_utils::scpt_native::native_function_name;
use encoding_rs::SHIFT_JIS;

const SINGLE_SCRIPT_SLOTS: usize = 20;
const PARTY_SCRIPT_SLOTS: usize = 80;

fn main() -> ExitCode {
    match run(env::args_os().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: impl Iterator<Item = OsString>) -> Result<(), CliError> {
    let values: Vec<OsString> = args.collect();
    let verbose = has_flag(&values, "--verbose");
    match (
        optional_flag_value(&values, "--quest-directory"),
        optional_flag_value(&values, "--file"),
        optional_flag_value(&values, "--decompile"),
    ) {
        (Some(directory), None, None) if !verbose => {
            let directory = PathBuf::from(directory);
            inspect_slots(&directory, "single", SINGLE_SCRIPT_SLOTS)?;
            inspect_slots(&directory, "party", PARTY_SCRIPT_SLOTS)
        }
        (None, Some(path), None) if !verbose => disassemble_file(&PathBuf::from(path)),
        (None, None, Some(path)) => decompile_file(&PathBuf::from(path), verbose),
        _ => Err(CliError::Usage),
    }
}

fn decompile_file(path: &Path, verbose: bool) -> Result<(), CliError> {
    let bytes = fs::read(path).map_err(|source| CliError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let script = Scpt::parse(&bytes).map_err(|source| CliError::Scpt {
        path: path.display().to_string(),
        source,
    })?;
    let control_flow = ControlFlowGraph::build(&script);
    control_flow
        .validate_stack_types(&script)
        .map_err(|source| CliError::StackFlow {
            path: path.display().to_string(),
            source,
        })?;
    println!("/* {} basic blocks */", control_flow.blocks.len());
    print!(
        "{}",
        scpt_decompile::decompile_with_options(&script, DecompileOptions { verbose })
    );
    Ok(())
}

fn inspect_slots(directory: &Path, prefix: &str, count: usize) -> Result<(), CliError> {
    for index in 0..count {
        let path = directory.join(format!("{prefix}{index:02}.dat"));
        if !path.try_exists().map_err(|source| CliError::Io {
            path: path.display().to_string(),
            source,
        })? {
            println!("{}: missing", path.display());
            continue;
        }

        let bytes = fs::read(&path).map_err(|source| CliError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let script = Scpt::parse(&bytes).map_err(|source| CliError::Scpt {
            path: path.display().to_string(),
            source,
        })?;
        println!(
            "{}: {} native functions, {} values, {} units, {} strings, {} instructions, {} bytes",
            path.display(),
            script.native_function_count,
            script.values.len(),
            script.units.len(),
            script.string_count(),
            script.instructions.len(),
            bytes.len()
        );
    }
    Ok(())
}

fn disassemble_file(path: &Path) -> Result<(), CliError> {
    let bytes = fs::read(path).map_err(|source| CliError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let script = Scpt::parse(&bytes).map_err(|source| CliError::Scpt {
        path: path.display().to_string(),
        source,
    })?;

    println!("file {}", path.display());
    println!(
        "header size=0x{:x} constant_08={} native_functions={} marker={:02x?}",
        script.header_size, script.constant_08, script.native_function_count, script.script_name
    );
    println!(
        "values offset=0x{:x} size=0x{:x} count={}",
        script.value_section_offset(),
        script.value_section_size,
        script.values.len()
    );
    for (index, value) in script.values.iter().enumerate() {
        let offset = script.value_section_offset() + (index as u64 * 6);
        let rendered = match value.kind {
            ScptValueKind::Integer => format!(
                "integer={} (0x{:08x})",
                value.raw_value as i32, value.raw_value
            ),
            ScptValueKind::Float => format!(
                "float={:?} (0x{:08x})",
                f32::from_bits(value.raw_value),
                value.raw_value
            ),
            ScptValueKind::String => match script.string_at(value.raw_value) {
                Some(bytes) => format!("string+0x{:x}={}", value.raw_value, render_text(bytes)),
                None => format!("string+0x{:x}=<invalid>", value.raw_value),
            },
        };
        println!(
            "  value {index:04} @0x{offset:08x}: kind={:?} attribute={} raw=0x{:08x} {rendered}",
            value.kind, value.attribute, value.raw_value
        );
    }

    println!(
        "units offset=0x{:x} size=0x{:x} count={}",
        script.unit_section_offset(),
        script.unit_section_size,
        script.units.len()
    );
    for (index, unit) in script.units.iter().enumerate() {
        let offset = script.unit_section_offset() + (index as u64 * 9);
        let range = match unit.kind {
            ScptUnitKind::InclusiveEnd => {
                format!("{}..={}", unit.first_instruction, unit.end_instruction)
            }
            ScptUnitKind::ExclusiveEnd => {
                format!("{}..{}", unit.first_instruction, unit.end_instruction)
            }
        };
        println!(
            "  unit {index:04} @0x{offset:08x}: kind={:?} instructions={range}",
            unit.kind
        );
    }

    println!(
        "strings offset=0x{:x} size=0x{:x} entries={}",
        script.string_section_offset(),
        script.string_section_size,
        script.string_count()
    );
    let mut string_offset = 0_usize;
    while string_offset < script.strings.len() {
        let remaining = &script.strings[string_offset..];
        let length = remaining
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(remaining.len());
        println!(
            "  string +0x{string_offset:04x}: {}",
            render_text(&remaining[..length])
        );
        string_offset += length + 1;
    }

    println!(
        "instructions offset=0x{:x} size=0x{:x} count={}",
        script.instruction_section_offset(),
        script.instruction_section_size,
        script.instructions.len()
    );
    for (index, instruction) in script.instructions.iter().enumerate() {
        let offset = script.instruction_section_offset() + (index as u64 * 12);
        println!(
            "  {index:05} @0x{offset:08x}: opcode={:02} operand_1=0x{:08x} operand_2=0x{:08x} {}",
            instruction.opcode,
            instruction.operand_1,
            instruction.operand_2,
            describe_instruction(&script, instruction)
        );
    }
    Ok(())
}

fn describe_instruction(script: &Scpt, instruction: &ScptInstruction) -> String {
    match instruction.kind() {
        ScptInstructionKind::NoOperation => "no-op".to_owned(),
        ScptInstructionKind::PushValue {
            value,
            value_type,
            indexed,
        } => format!(
            "push {} value=0x{value:x}{}",
            value_type_name(value_type),
            index_suffix(indexed)
        ),
        ScptInstructionKind::Reserved => "reserved-or-invalid".to_owned(),
        ScptInstructionKind::Jump { target } => format!("jump {target}"),
        ScptInstructionKind::StoreValue { value, indexed } => {
            format!("store value={value}{}", index_suffix(indexed))
        }
        ScptInstructionKind::BinaryOperator(operator) => {
            format!("binary {}", binary_operator_name(operator))
        }
        ScptInstructionKind::UnaryOperator(operator) => {
            format!("unary {}", unary_operator_name(operator))
        }
        ScptInstructionKind::AssignmentOperator {
            value,
            indexed,
            operator,
        } => format!(
            "assign {} value={value}{}",
            assignment_operator_name(operator),
            index_suffix(indexed)
        ),
        ScptInstructionKind::Increment { value, indexed } => {
            format!("increment value={value}{}", index_suffix(indexed))
        }
        ScptInstructionKind::Decrement { value, indexed } => {
            format!("decrement value={value}{}", index_suffix(indexed))
        }
        ScptInstructionKind::Branch {
            true_target,
            false_target,
        } => format!("branch true={true_target} false={false_target}"),
        ScptInstructionKind::EqualBranch {
            equal_target,
            unequal_target,
        } => format!("branch-equal equal={equal_target} unequal={unequal_target}"),
        ScptInstructionKind::Return => "return".to_owned(),
        ScptInstructionKind::ReturnValue => "return-value".to_owned(),
        ScptInstructionKind::NativeCall {
            function,
            argument_count,
            result_value,
        } => match native_function_name(function) {
            Some(name) => format!(
                "native {function} {name} argc={argument_count}{}",
                result_suffix(script, result_value)
            ),
            None => format!(
                "native {function} argc={argument_count}{}",
                result_suffix(script, result_value)
            ),
        },
        ScptInstructionKind::UnitCall { unit } => format!("call-unit {unit}"),
    }
}

fn value_type_name(value_type: u16) -> String {
    match u8::try_from(value_type).ok() {
        Some(b'A') => "stored".to_owned(),
        Some(b'F') => "float".to_owned(),
        Some(b'I') => "integer".to_owned(),
        _ => format!("type=0x{value_type:04x}"),
    }
}

fn index_suffix(indexed: bool) -> &'static str {
    if indexed { " indexed" } else { "" }
}

fn binary_operator_name(operator: ScptBinaryOperator) -> &'static str {
    match operator {
        ScptBinaryOperator::Add => "+",
        ScptBinaryOperator::Subtract => "-",
        ScptBinaryOperator::Multiply => "*",
        ScptBinaryOperator::Divide => "/",
        ScptBinaryOperator::Modulo => "%",
        ScptBinaryOperator::LessThan => "<",
        ScptBinaryOperator::LessThanOrEqual => "<=",
        ScptBinaryOperator::GreaterThan => ">",
        ScptBinaryOperator::GreaterThanOrEqual => ">=",
        ScptBinaryOperator::Equal => "==",
        ScptBinaryOperator::NotEqual => "!=",
        ScptBinaryOperator::LogicalAnd => "&&",
        ScptBinaryOperator::LogicalOr => "||",
        ScptBinaryOperator::BitwiseAnd => "&",
        ScptBinaryOperator::BitwiseOr => "|",
        ScptBinaryOperator::BitwiseXor => "^",
        ScptBinaryOperator::ShiftLeft => "<<",
        ScptBinaryOperator::ShiftRight => ">>",
    }
}

fn unary_operator_name(operator: ScptUnaryOperator) -> &'static str {
    match operator {
        ScptUnaryOperator::Negate => "-",
        ScptUnaryOperator::LogicalNot => "!",
        ScptUnaryOperator::BitwiseNot => "~",
    }
}

fn assignment_operator_name(operator: ScptAssignmentOperator) -> &'static str {
    match operator {
        ScptAssignmentOperator::Add => "+=",
        ScptAssignmentOperator::Subtract => "-=",
        ScptAssignmentOperator::Multiply => "*=",
        ScptAssignmentOperator::Divide => "/=",
        ScptAssignmentOperator::Modulo => "%=",
        ScptAssignmentOperator::BitwiseAnd => "&=",
        ScptAssignmentOperator::BitwiseOr => "|=",
        ScptAssignmentOperator::BitwiseXor => "^=",
        ScptAssignmentOperator::ShiftLeft => "<<=",
        ScptAssignmentOperator::ShiftRight => ">>=",
    }
}

fn result_suffix(script: &Scpt, result_value: Option<u32>) -> String {
    result_value.map_or_else(String::new, |value| {
        let value_kind = usize::try_from(value)
            .ok()
            .and_then(|index| script.values.get(index))
            .map_or("invalid", |entry| match entry.kind {
                ScptValueKind::Integer => "integer",
                ScptValueKind::Float => "float",
                ScptValueKind::String => "string",
            });
        format!(" result=value[{value}]:{value_kind}")
    })
}

fn render_text(bytes: &[u8]) -> String {
    let (text, _, had_errors) = SHIFT_JIS.decode(bytes);
    if had_errors {
        bytes
            .iter()
            .map(|value| format!("{value:02x}"))
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        format!("{text:?}")
    }
}

fn optional_flag_value<'a>(values: &'a [OsString], flag: &str) -> Option<&'a OsStr> {
    values
        .windows(2)
        .find(|pair| pair[0] == OsStr::new(flag))
        .map(|pair| pair[1].as_os_str())
}

fn has_flag(values: &[OsString], flag: &str) -> bool {
    values.iter().any(|value| value == OsStr::new(flag))
}

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error(
        "usage: scpt-inspect <--quest-directory DIRECTORY|--file FILE|--decompile FILE [--verbose]>"
    )]
    Usage,
    #[error("cannot read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot inspect {path}: {source}")]
    Scpt {
        path: String,
        #[source]
        source: ScptError,
    },
    #[error("cannot analyze {path}: {source}")]
    StackFlow {
        path: String,
        #[source]
        source: StackFlowError,
    },
}
