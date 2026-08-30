use std::collections::{BTreeSet, HashMap};
use std::fmt::Write;

use crate::scpt::{
    Scpt, ScptAssignmentOperator, ScptBinaryOperator, ScptInstructionKind, ScptUnaryOperator,
    ScptUnit, ScptUnitKind,
};
use crate::scpt_native::native_function_name;

pub fn decompile(script: &Scpt) -> String {
    decompile_with_options(script, DecompileOptions::default())
}

pub fn decompile_with_options(script: &Scpt, options: DecompileOptions) -> String {
    Decompiler::new(script, options).run()
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DecompileOptions {
    pub verbose: bool,
}

struct Decompiler<'a> {
    script: &'a Scpt,
    labels: BTreeSet<usize>,
    units_by_start: HashMap<usize, UnitDefinition>,
    structured_loops: HashMap<usize, StructuredLoop>,
    structured_branches: HashMap<usize, StructuredBranch>,
    suppressed_jumps: BTreeSet<usize>,
    active_regions: Vec<ActiveRegion>,
    stack: Vec<String>,
    pending_native_results: HashMap<u32, String>,
    inline_native_results: BTreeSet<usize>,
    missing_stack_values: usize,
    indent: usize,
    active_unit_end: Option<usize>,
    pending_else_end: Option<usize>,
    redundant_unit_returns: BTreeSet<usize>,
    options: DecompileOptions,
    output: String,
}

impl<'a> Decompiler<'a> {
    fn new(script: &'a Scpt, options: DecompileOptions) -> Self {
        let units_by_start = script
            .units
            .iter()
            .enumerate()
            .filter_map(|(unit, range)| {
                let start = usize::try_from(range.first_instruction).ok()?;
                let end = unit_end(range)?;
                Some((start, UnitDefinition { unit, end }))
            })
            .collect::<HashMap<_, _>>();
        let structured_loops = find_structured_loops(script);
        let structured_branches = find_structured_branches(script);
        let suppressed_jumps = structured_loops
            .values()
            .map(|structured_loop| structured_loop.back_jump)
            .chain(
                structured_branches
                    .values()
                    .flat_map(|branch| branch.suppressed_jumps.iter().copied()),
            )
            .collect::<BTreeSet<_>>();
        let mut labels = BTreeSet::new();
        for (index, instruction) in script.instructions.iter().enumerate() {
            if structured_loops.contains_key(&index)
                || structured_branches.contains_key(&index)
                || suppressed_jumps.contains(&index)
            {
                continue;
            }
            match instruction.kind() {
                ScptInstructionKind::Jump { target } => insert_label(&mut labels, target),
                ScptInstructionKind::Branch {
                    true_target,
                    false_target,
                } => {
                    insert_label(&mut labels, true_target);
                    insert_label(&mut labels, false_target);
                }
                ScptInstructionKind::EqualBranch {
                    equal_target,
                    unequal_target,
                } => {
                    insert_label(&mut labels, equal_target);
                    insert_label(&mut labels, unequal_target);
                }
                _ => {}
            }
        }
        for start in units_by_start.keys() {
            labels.remove(start);
        }
        Self {
            script,
            labels,
            units_by_start,
            structured_loops,
            structured_branches,
            suppressed_jumps,
            active_regions: Vec::new(),
            stack: Vec::new(),
            pending_native_results: HashMap::new(),
            inline_native_results: find_inline_native_results(script),
            missing_stack_values: 0,
            indent: 0,
            active_unit_end: None,
            pending_else_end: None,
            redundant_unit_returns: find_redundant_unit_returns(script),
            options,
            output: String::new(),
        }
    }

    fn run(mut self) -> String {
        for (index, instruction) in self.script.instructions.iter().enumerate() {
            self.close_regions_at(index);
            self.close_unit_at(index);
            self.open_unit_at(index);
            if self.labels.contains(&index) {
                self.flush_pending_else();
                if !self.stack.is_empty() {
                    let _ = writeln!(
                        self.output,
                        "    /* discarded stack at block boundary: {} */",
                        self.stack.join(", ")
                    );
                    self.stack.clear();
                }
                self.missing_stack_values = 0;
                self.pending_native_results.clear();
                let _ = writeln!(self.output, "{}label_{index}:", "    ".repeat(self.indent));
            }
            self.instruction(index, instruction.kind(), instruction.opcode);
        }
        self.close_regions_at(self.script.instructions.len());
        self.close_unit_at(self.script.instructions.len());
        self.output
    }

    fn instruction(&mut self, index: usize, kind: ScptInstructionKind, opcode: u32) {
        match kind {
            ScptInstructionKind::NoOperation => {
                if opcode != 0 {
                    self.line(index, &format!("opcode_{opcode};"));
                }
            }
            ScptInstructionKind::PushValue {
                value,
                value_type,
                indexed,
            } => {
                let index_expression = indexed.then(|| self.pop());
                let expression = match u8::try_from(value_type).ok() {
                    Some(b'A') if index_expression.is_none() => self
                        .pending_native_results
                        .get(&value)
                        .cloned()
                        .unwrap_or_else(|| value_expression(value, None)),
                    Some(b'A') => value_expression(value, index_expression.as_deref()),
                    Some(b'F') => format_float(value),
                    Some(b'I') => format!("{}", value as i32),
                    _ => format!("raw_value(0x{value_type:04x}, 0x{value:08x})"),
                };
                self.stack.push(expression);
                if self.inline_native_results.contains(&index) {
                    self.pending_native_results.remove(&value);
                }
            }
            ScptInstructionKind::Reserved => self.line(
                index,
                &format!("raw_opcode_{opcode}; /* invalid or reserved */"),
            ),
            ScptInstructionKind::Jump { target } => {
                if self.suppressed_jumps.contains(&index) {
                    self.stack.clear();
                    return;
                }
                self.line(index, &format!("goto {};", self.target_name(target)));
                self.stack.clear();
            }
            ScptInstructionKind::StoreValue { value, indexed } => {
                let assigned = self.pop();
                let index_expression = indexed.then(|| self.pop());
                if indexed {
                    self.pending_native_results.clear();
                } else {
                    self.pending_native_results.remove(&value);
                }
                self.line(
                    index,
                    &format!(
                        "{} = {assigned};",
                        value_expression(value, index_expression.as_deref())
                    ),
                );
            }
            ScptInstructionKind::BinaryOperator(operator) => {
                let right = self.pop();
                let left = self.pop();
                self.stack
                    .push(format!("({left} {} {right})", binary_operator(operator)));
            }
            ScptInstructionKind::UnaryOperator(operator) => {
                let value = self.pop();
                self.stack
                    .push(format!("({}{value})", unary_operator(operator)));
            }
            ScptInstructionKind::AssignmentOperator {
                value,
                indexed,
                operator,
            } => {
                let assigned = self.pop();
                let index_expression = indexed.then(|| self.pop());
                self.pending_native_results.remove(&value);
                self.line(
                    index,
                    &format!(
                        "{} {} {assigned};",
                        value_expression(value, index_expression.as_deref()),
                        assignment_operator(operator)
                    ),
                );
            }
            ScptInstructionKind::Increment { value, indexed } => {
                let index_expression = indexed.then(|| self.pop());
                self.pending_native_results.remove(&value);
                self.line(
                    index,
                    &format!(
                        "{}++;",
                        value_expression(value, index_expression.as_deref())
                    ),
                );
            }
            ScptInstructionKind::Decrement { value, indexed } => {
                let index_expression = indexed.then(|| self.pop());
                self.pending_native_results.remove(&value);
                self.line(
                    index,
                    &format!(
                        "{}--;",
                        value_expression(value, index_expression.as_deref())
                    ),
                );
            }
            ScptInstructionKind::Branch {
                true_target,
                false_target,
            } => {
                let condition = self.pop();
                if let Some(structured_loop) = self.structured_loops.get(&index).cloned() {
                    let condition = if structured_loop.body_on_true {
                        condition
                    } else {
                        format!("!({})", strip_outer_parentheses(&condition))
                    };
                    self.open_structured_loop(index, &condition, structured_loop);
                    return;
                }
                if let Some(branch) = self.structured_branches.get(&index).cloned() {
                    let condition = if branch.body_on_true {
                        condition
                    } else {
                        format!("!({})", strip_outer_parentheses(&condition))
                    };
                    self.open_structured_branch(index, &condition, branch);
                    return;
                }
                self.line(
                    index,
                    &format!(
                        "if {condition} goto {}; else goto {};",
                        self.target_name(true_target),
                        self.target_name(false_target)
                    ),
                );
                self.stack.clear();
            }
            ScptInstructionKind::EqualBranch {
                equal_target,
                unequal_target,
            } => {
                let right = self.pop();
                let left = self.pop();
                if let Some(structured_loop) = self.structured_loops.get(&index).cloned() {
                    let condition = if structured_loop.body_on_true {
                        format!("{left} == {right}")
                    } else {
                        format!("{left} != {right}")
                    };
                    self.open_structured_loop(index, &condition, structured_loop);
                    return;
                }
                if let Some(branch) = self.structured_branches.get(&index).cloned() {
                    let condition = if branch.body_on_true {
                        format!("{left} == {right}")
                    } else {
                        format!("{left} != {right}")
                    };
                    self.open_structured_branch(index, &condition, branch);
                    return;
                }
                self.line(
                    index,
                    &format!(
                        "if {left} == {right} goto {}; else goto {};",
                        self.target_name(equal_target),
                        self.target_name(unequal_target)
                    ),
                );
                self.stack.clear();
            }
            ScptInstructionKind::Return => {
                if !self.redundant_unit_returns.contains(&index) {
                    self.line(index, "return;");
                }
                self.stack.clear();
            }
            ScptInstructionKind::ReturnValue => {
                let value = self.pop();
                self.line(index, &format!("return {value};"));
                self.stack.clear();
            }
            ScptInstructionKind::NativeCall {
                function,
                argument_count,
                result_value,
            } => {
                let mut arguments = (0..argument_count).map(|_| self.pop()).collect::<Vec<_>>();
                arguments.reverse();
                let function_name = native_function_name(function)
                    .map_or_else(|| format!("native_{function}"), str::to_owned);
                let call = format!("{function_name}({})", arguments.join(", "));
                if let Some(value) =
                    result_value.filter(|_| self.inline_native_results.contains(&(index + 1)))
                {
                    self.pending_native_results.insert(value, call);
                    return;
                }
                let statement = result_value.map_or_else(
                    || format!("{call};"),
                    |value| {
                        let result = format!("result_{index}");
                        self.pending_native_results.insert(value, result.clone());
                        format!("let {result} = {call}; value[{value}] = {result};")
                    },
                );
                self.line(index, &statement);
            }
            ScptInstructionKind::UnitCall { unit } => {
                self.pending_native_results.clear();
                let stack = std::mem::take(&mut self.stack);
                if stack.is_empty() {
                    self.line(index, &format!("unit_{unit}();"));
                } else {
                    self.line(
                        index,
                        &format!("unit_{unit}(with_stack({}));", stack.join(", ")),
                    );
                }
            }
        }
    }

    fn pop(&mut self) -> String {
        self.stack.pop().unwrap_or_else(|| {
            let index = self.missing_stack_values;
            self.missing_stack_values += 1;
            format!("incoming_stack[{index}]")
        })
    }

    fn target_name(&self, target: u32) -> String {
        usize::try_from(target)
            .ok()
            .and_then(|target| self.units_by_start.get(&target))
            .map_or_else(
                || format!("label_{target}"),
                |unit| format!("unit_{}", unit.unit),
            )
    }

    fn line(&mut self, instruction: usize, text: &str) {
        self.flush_pending_else();
        self.write_line(instruction, text);
    }

    fn write_line(&mut self, instruction: usize, text: &str) {
        let indentation = "    ".repeat(self.indent);
        if self.options.verbose {
            let file_offset = self.script.instruction_section_offset() + instruction as u64 * 12;
            let _ = writeln!(
                self.output,
                "{indentation}{text} /* instruction {instruction}, file 0x{file_offset:x} */"
            );
        } else {
            let _ = writeln!(self.output, "{indentation}{text}");
        }
    }

    fn open_unit_at(&mut self, instruction: usize) {
        let Some(unit) = self.units_by_start.get(&instruction).copied() else {
            return;
        };
        self.flush_pending_else();
        let indentation = "    ".repeat(self.indent);
        let _ = writeln!(self.output, "{indentation}fn unit_{}() {{", unit.unit);
        self.indent += 1;
        self.active_unit_end = Some(unit.end);
        self.stack.clear();
        self.pending_native_results.clear();
    }

    fn close_unit_at(&mut self, instruction: usize) {
        if self.active_unit_end != Some(instruction) {
            return;
        }
        self.flush_pending_else();
        self.indent = self.indent.saturating_sub(1);
        let indentation = "    ".repeat(self.indent);
        let _ = writeln!(self.output, "{indentation}}}");
        self.active_unit_end = None;
        self.stack.clear();
        self.pending_native_results.clear();
    }

    fn open_structured_branch(
        &mut self,
        instruction: usize,
        condition: &str,
        branch: StructuredBranch,
    ) {
        let is_else_if = self
            .pending_else_end
            .is_some_and(|parent_end| self.empty_region_tail(branch.end, parent_end));
        let keyword = if is_else_if {
            self.pending_else_end = None;
            self.active_regions.pop();
            "} else if"
        } else {
            self.flush_pending_else();
            "if"
        };
        self.write_line(
            instruction,
            &format!("{keyword} ({}) {{", strip_outer_parentheses(condition)),
        );
        self.indent += 1;
        self.active_regions.push(ActiveRegion::from(branch));
        self.stack.clear();
    }

    fn open_structured_loop(
        &mut self,
        instruction: usize,
        condition: &str,
        structured_loop: StructuredLoop,
    ) {
        self.line(
            instruction,
            &format!("while ({}) {{", strip_outer_parentheses(condition)),
        );
        self.indent += 1;
        self.active_regions.push(ActiveRegion {
            else_start: None,
            end: structured_loop.end,
            in_else: false,
        });
        self.stack.clear();
    }

    fn empty_region_tail(&self, start: usize, end: usize) -> bool {
        start <= end
            && self.labels.range(start..end).next().is_none()
            && (start..end).all(|instruction| {
                self.suppressed_jumps.contains(&instruction)
                    || matches!(
                        self.script.instructions[instruction].kind(),
                        ScptInstructionKind::NoOperation
                    )
            })
    }

    fn flush_pending_else(&mut self) {
        if self.pending_else_end.take().is_none() {
            return;
        }
        let indentation = "    ".repeat(self.indent);
        let _ = writeln!(self.output, "{indentation}}} else {{");
        self.indent += 1;
        if let Some(region) = self.active_regions.last_mut() {
            region.in_else = true;
        }
    }

    fn close_regions_at(&mut self, instruction: usize) {
        if self.pending_else_end == Some(instruction) {
            self.flush_pending_else();
        }
        while let Some(region) = self.active_regions.last() {
            if region.else_start == Some(instruction) && !region.in_else {
                self.indent -= 1;
                self.pending_else_end = Some(region.end);
                self.stack.clear();
                self.pending_native_results.clear();
                break;
            }
            if region.end != instruction {
                break;
            }
            self.indent -= 1;
            let indentation = "    ".repeat(self.indent);
            let _ = writeln!(self.output, "{indentation}}}");
            self.active_regions.pop();
            self.stack.clear();
            self.pending_native_results.clear();
        }
    }
}

#[derive(Clone, Debug)]
struct StructuredBranch {
    body_on_true: bool,
    else_start: Option<usize>,
    end: usize,
    suppressed_jumps: Vec<usize>,
}

#[derive(Clone, Debug)]
struct StructuredLoop {
    body_on_true: bool,
    end: usize,
    back_jump: usize,
}

struct ActiveRegion {
    else_start: Option<usize>,
    end: usize,
    in_else: bool,
}

#[derive(Clone, Copy)]
struct UnitDefinition {
    unit: usize,
    end: usize,
}

impl From<StructuredBranch> for ActiveRegion {
    fn from(branch: StructuredBranch) -> Self {
        Self {
            else_start: branch.else_start,
            end: branch.end,
            in_else: false,
        }
    }
}

fn find_structured_loops(script: &Scpt) -> HashMap<usize, StructuredLoop> {
    let control_flow = ControlFlowIndex::new(script);
    let inline_native_results = find_inline_native_results(script);
    let mut candidates = script
        .instructions
        .iter()
        .enumerate()
        .filter_map(|(branch, instruction)| {
            let (true_target, false_target) = match instruction.kind() {
                ScptInstructionKind::Branch {
                    true_target,
                    false_target,
                } => (true_target, false_target),
                ScptInstructionKind::EqualBranch {
                    equal_target,
                    unequal_target,
                } => (equal_target, unequal_target),
                _ => return None,
            };
            structured_loop_candidate(
                script,
                &control_flow,
                &inline_native_results,
                branch,
                true_target,
                false_target,
            )
            .map(|(header, structured_loop)| (header, branch, structured_loop))
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|(header, _, structured_loop)| {
        (*header, std::cmp::Reverse(structured_loop.end))
    });

    let mut accepted = Vec::<(usize, usize)>::new();
    let mut result = HashMap::new();
    for (header, branch, structured_loop) in candidates {
        while accepted.last().is_some_and(|(_, end)| header >= *end) {
            accepted.pop();
        }
        if accepted
            .last()
            .is_some_and(|(_, end)| structured_loop.end > *end)
        {
            continue;
        }
        accepted.push((header, structured_loop.end));
        result.insert(branch, structured_loop);
    }
    result
}

fn structured_loop_candidate(
    script: &Scpt,
    control_flow: &ControlFlowIndex,
    inline_native_results: &BTreeSet<usize>,
    branch: usize,
    true_target: u32,
    false_target: u32,
) -> Option<(usize, StructuredLoop)> {
    let true_target = usize::try_from(true_target).ok()?;
    let false_target = usize::try_from(false_target).ok()?;
    let (body_on_true, body_start, end) = if true_target < false_target {
        (true, true_target, false_target)
    } else {
        (false, false_target, true_target)
    };
    if body_start <= branch
        || end <= body_start
        || end > script.instructions.len()
        || !(branch + 1..body_start).all(|index| {
            matches!(
                script.instructions[index].kind(),
                ScptInstructionKind::NoOperation
            )
        })
    {
        return None;
    }

    let back_jump = last_effective_instruction(script, body_start, end)?;
    let ScptInstructionKind::Jump { target: header } = script.instructions[back_jump].kind() else {
        return None;
    };
    let header = usize::try_from(header).ok()?;
    if header >= branch
        || !(header..branch)
            .all(|index| is_silent_condition_instruction(script, inline_native_results, index))
        || control_flow.has_external_entry(header, end)
        || loop_body_has_unsupported_edge(script, body_start, end, back_jump, header)
    {
        return None;
    }

    Some((
        header,
        StructuredLoop {
            body_on_true,
            end,
            back_jump,
        },
    ))
}

fn is_silent_condition_instruction(
    script: &Scpt,
    inline_native_results: &BTreeSet<usize>,
    instruction: usize,
) -> bool {
    match script.instructions[instruction].kind() {
        ScptInstructionKind::NoOperation
        | ScptInstructionKind::PushValue { .. }
        | ScptInstructionKind::BinaryOperator(_)
        | ScptInstructionKind::UnaryOperator(_) => true,
        ScptInstructionKind::NativeCall { .. } => {
            inline_native_results.contains(&(instruction + 1))
        }
        _ => false,
    }
}

fn loop_body_has_unsupported_edge(
    script: &Scpt,
    body_start: usize,
    end: usize,
    back_jump: usize,
    header: usize,
) -> bool {
    (body_start..end).any(
        |instruction| match script.instructions[instruction].kind() {
            ScptInstructionKind::Jump { target } => usize::try_from(target).is_ok_and(|target| {
                if instruction == back_jump {
                    target != header
                } else {
                    target < body_start || target >= end
                }
            }),
            ScptInstructionKind::Branch {
                true_target,
                false_target,
            } => [true_target, false_target].into_iter().any(|target| {
                usize::try_from(target).is_ok_and(|target| target < body_start || target >= end)
            }),
            ScptInstructionKind::EqualBranch {
                equal_target,
                unequal_target,
            } => [equal_target, unequal_target].into_iter().any(|target| {
                usize::try_from(target).is_ok_and(|target| target < body_start || target >= end)
            }),
            _ => false,
        },
    )
}

fn find_structured_branches(script: &Scpt) -> HashMap<usize, StructuredBranch> {
    let control_flow = ControlFlowIndex::new(script);
    let mut candidates = script
        .instructions
        .iter()
        .enumerate()
        .filter_map(|(index, instruction)| {
            let (true_target, false_target) = match instruction.kind() {
                ScptInstructionKind::Branch {
                    true_target,
                    false_target,
                } => (true_target, false_target),
                ScptInstructionKind::EqualBranch {
                    equal_target,
                    unequal_target,
                } => (equal_target, unequal_target),
                _ => return None,
            };
            structured_branch_candidate(script, &control_flow, index, true_target, false_target)
                .map(|branch| (index, branch))
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|(start, branch)| (*start, std::cmp::Reverse(branch.end)));

    let mut accepted = Vec::<(usize, usize)>::new();
    let mut result = HashMap::new();
    for (start, branch) in candidates {
        while accepted.last().is_some_and(|(_, end)| start >= *end) {
            accepted.pop();
        }
        if accepted.last().is_some_and(|(_, end)| branch.end > *end) {
            continue;
        }
        accepted.push((start, branch.end));
        result.insert(start, branch);
    }
    result
}

fn structured_branch_candidate(
    script: &Scpt,
    control_flow: &ControlFlowIndex,
    instruction: usize,
    true_target: u32,
    false_target: u32,
) -> Option<StructuredBranch> {
    let true_target = usize::try_from(true_target).ok()?;
    let false_target = usize::try_from(false_target).ok()?;
    let (body_on_true, split) = if true_target == instruction + 1 && false_target > true_target {
        (true, false_target)
    } else if false_target == instruction + 1 && true_target > false_target {
        (false, true_target)
    } else {
        return None;
    };

    let mut else_start = None;
    let mut end = split;
    let mut suppressed_jumps = Vec::new();
    if let Some(last_then) = last_effective_instruction(script, instruction + 1, split)
        && let ScptInstructionKind::Jump { target } = script.instructions[last_then].kind()
        && let Ok(target) = usize::try_from(target)
    {
        if target > split && target <= script.instructions.len() {
            else_start = Some(split);
            end = target;
            suppressed_jumps.push(last_then);
        } else if target == split {
            suppressed_jumps.push(last_then);
        }
    }
    if let Some(last_else) =
        else_start.and_then(|start| last_effective_instruction(script, start, end))
        && let ScptInstructionKind::Jump { target } = script.instructions[last_else].kind()
        && usize::try_from(target).ok() == Some(end)
    {
        suppressed_jumps.push(last_else);
    }
    if end > script.instructions.len()
        || control_flow.has_external_entry(instruction, end)
        || control_flow.has_jump_outside_region(instruction + 1, end)
    {
        return None;
    }
    Some(StructuredBranch {
        body_on_true,
        else_start,
        end,
        suppressed_jumps,
    })
}

fn last_effective_instruction(script: &Scpt, start: usize, end: usize) -> Option<usize> {
    (start..end).rev().find(|index| {
        !matches!(
            script.instructions[*index].kind(),
            ScptInstructionKind::NoOperation
        )
    })
}

fn unit_end(unit: &ScptUnit) -> Option<usize> {
    let end = usize::try_from(unit.end_instruction).ok()?;
    match unit.kind {
        ScptUnitKind::InclusiveEnd => end.checked_add(1),
        ScptUnitKind::ExclusiveEnd => Some(end),
    }
}

fn find_redundant_unit_returns(script: &Scpt) -> BTreeSet<usize> {
    script
        .units
        .iter()
        .filter_map(|unit| {
            let start = usize::try_from(unit.first_instruction).ok()?;
            let terminal = unit_end(unit)?.checked_sub(1)?;
            if !matches!(
                script.instructions.get(terminal)?.kind(),
                ScptInstructionKind::Return
            ) {
                return None;
            }
            let previous = last_effective_instruction(script, start, terminal)?;
            if !matches!(
                script.instructions[previous].kind(),
                ScptInstructionKind::Return | ScptInstructionKind::ReturnValue
            ) || script
                .instructions
                .iter()
                .any(|instruction| instruction_targets(instruction.kind(), terminal))
            {
                return None;
            }
            Some(terminal)
        })
        .collect()
}

fn instruction_targets(kind: ScptInstructionKind, target: usize) -> bool {
    match kind {
        ScptInstructionKind::Jump { target: candidate } => {
            usize::try_from(candidate).ok() == Some(target)
        }
        ScptInstructionKind::Branch {
            true_target,
            false_target,
        } => [true_target, false_target]
            .into_iter()
            .any(|candidate| usize::try_from(candidate).ok() == Some(target)),
        ScptInstructionKind::EqualBranch {
            equal_target,
            unequal_target,
        } => [equal_target, unequal_target]
            .into_iter()
            .any(|candidate| usize::try_from(candidate).ok() == Some(target)),
        _ => false,
    }
}

struct ControlFlowIndex {
    incoming_sources: RangeExtrema,
    jump_targets: RangeExtrema,
    unit_entries: BTreeSet<usize>,
}

impl ControlFlowIndex {
    fn new(script: &Scpt) -> Self {
        let instruction_count = script.instructions.len();
        let mut incoming_sources = RangeExtrema::new(instruction_count + 1);
        let mut jump_targets = RangeExtrema::new(instruction_count);
        for (source, instruction) in script.instructions.iter().enumerate() {
            match instruction.kind() {
                ScptInstructionKind::Jump { target } => {
                    if let Ok(target) = usize::try_from(target) {
                        incoming_sources.insert(target, source);
                        jump_targets.insert(source, target);
                    }
                }
                ScptInstructionKind::Branch {
                    true_target,
                    false_target,
                } => {
                    for target in [true_target, false_target] {
                        if let Ok(target) = usize::try_from(target) {
                            incoming_sources.insert(target, source);
                        }
                    }
                }
                ScptInstructionKind::EqualBranch {
                    equal_target,
                    unequal_target,
                } => {
                    for target in [equal_target, unequal_target] {
                        if let Ok(target) = usize::try_from(target) {
                            incoming_sources.insert(target, source);
                        }
                    }
                }
                _ => {}
            }
        }
        Self {
            incoming_sources,
            jump_targets,
            unit_entries: script
                .units
                .iter()
                .filter_map(|unit| usize::try_from(unit.first_instruction).ok())
                .collect(),
        }
    }

    fn has_external_entry(&self, branch: usize, end: usize) -> bool {
        let (minimum, maximum) = self.incoming_sources.query(branch + 1, end);
        minimum < branch
            || maximum >= end
            || self.unit_entries.range(branch + 1..end).next().is_some()
    }

    fn has_jump_outside_region(&self, start: usize, end: usize) -> bool {
        let (minimum, maximum) = self.jump_targets.query(start, end);
        minimum < start || maximum > end
    }
}

struct RangeExtrema {
    leaf_count: usize,
    minima: Vec<usize>,
    maxima: Vec<usize>,
}

impl RangeExtrema {
    fn new(item_count: usize) -> Self {
        let leaf_count = item_count.next_power_of_two();
        Self {
            leaf_count,
            minima: vec![usize::MAX; leaf_count * 2],
            maxima: vec![0; leaf_count * 2],
        }
    }

    fn insert(&mut self, index: usize, value: usize) {
        if index >= self.leaf_count {
            return;
        }
        let mut node = self.leaf_count + index;
        self.minima[node] = self.minima[node].min(value);
        self.maxima[node] = self.maxima[node].max(value);
        while node > 1 {
            node /= 2;
            self.minima[node] = self.minima[node * 2].min(self.minima[node * 2 + 1]);
            self.maxima[node] = self.maxima[node * 2].max(self.maxima[node * 2 + 1]);
        }
    }

    fn query(&self, start: usize, end: usize) -> (usize, usize) {
        let mut start = start + self.leaf_count;
        let mut end = end + self.leaf_count;
        let mut minimum = usize::MAX;
        let mut maximum = 0;
        while start < end {
            if start % 2 == 1 {
                minimum = minimum.min(self.minima[start]);
                maximum = maximum.max(self.maxima[start]);
                start += 1;
            }
            if end % 2 == 1 {
                end -= 1;
                minimum = minimum.min(self.minima[end]);
                maximum = maximum.max(self.maxima[end]);
            }
            start /= 2;
            end /= 2;
        }
        (minimum, maximum)
    }
}

fn find_inline_native_results(script: &Scpt) -> BTreeSet<usize> {
    script
        .instructions
        .iter()
        .enumerate()
        .filter_map(|(index, instruction)| {
            let ScptInstructionKind::NativeCall {
                result_value: Some(result_value),
                ..
            } = instruction.kind()
            else {
                return None;
            };
            let next = script.instructions.get(index + 1)?;
            let ScptInstructionKind::PushValue {
                value,
                value_type,
                indexed: false,
            } = next.kind()
            else {
                return None;
            };
            (value == result_value && value_type == u16::from(b'A')).then_some(index + 1)
        })
        .collect()
}

fn insert_label(labels: &mut BTreeSet<usize>, target: u32) {
    if let Ok(target) = usize::try_from(target) {
        labels.insert(target);
    }
}

fn value_expression(value: u32, index: Option<&str>) -> String {
    index.map_or_else(
        || format!("value[{value}]"),
        |index| format!("value[{value} + {index}]"),
    )
}

fn strip_outer_parentheses(expression: &str) -> &str {
    let Some(inner) = expression
        .strip_prefix('(')
        .and_then(|value| value.strip_suffix(')'))
    else {
        return expression;
    };
    let mut depth = 0_u32;
    for (index, character) in expression.char_indices() {
        match character {
            '(' => depth += 1,
            ')' if depth == 0 => return expression,
            ')' => depth -= 1,
            _ => {}
        }
        if depth == 0 && index + character.len_utf8() < expression.len() {
            return expression;
        }
    }
    if depth == 0 { inner } else { expression }
}

fn format_float(bits: u32) -> String {
    let value = f32::from_bits(bits);
    if value.is_finite() {
        format!("{value:?}")
    } else {
        format!("f32::from_bits(0x{bits:08x})")
    }
}

fn binary_operator(operator: ScptBinaryOperator) -> &'static str {
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

fn unary_operator(operator: ScptUnaryOperator) -> &'static str {
    match operator {
        ScptUnaryOperator::Negate => "-",
        ScptUnaryOperator::LogicalNot => "!",
        ScptUnaryOperator::BitwiseNot => "~",
    }
}

fn assignment_operator(operator: ScptAssignmentOperator) -> &'static str {
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
