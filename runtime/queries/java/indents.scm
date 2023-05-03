[
  (argument_list)
  (array_initializer)
  (block)
  (class_body)
  (enum_body)
  (formal_parameters)
  (interface_body)
  (method_body)
  (record_declaration)
  (switch_block)
  (type_arguments)
  (type_parameters)
] @indent.begin

(expression_statement (method_invocation) @indent.begin)

[
  "("
  ")"
  "{"
  "}"
  "["
  "]"
] @indent.branch

"}" @indent.end

(line_comment) @indent.ignore

[
  (ERROR)
  (block_comment)
] @indent.auto

