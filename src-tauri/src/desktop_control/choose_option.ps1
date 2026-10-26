# Included only after the existing exact ComboBox target preflight. The option
# is approval-bound data in an environment variable, never executable script.
$optionLabel = [string]$env:ANGELBOT_CHOICE_OPTION
if ([string]::IsNullOrWhiteSpace($optionLabel) -or $optionLabel.Length -gt 80 -or
    $optionLabel -cne $optionLabel.Trim() -or $optionLabel -match '\p{Cc}') {
  throw 'INVALID_ARGUMENT|An exact bounded option label is required'
}
$comboPattern = Assert-UiaControlTarget $target
$before = Get-UiaControlOperationState $comboPattern
if ($before -cnotin @('Collapsed', 'Expanded')) {
  throw 'UNSUPPORTED_CONTROL|The dropdown expansion state is unsupported'
}
function Get-ChoiceSelectionPattern {
  $null = Assert-UiaControlTarget $target
  $selection = $null
  if (-not $target.TryGetCurrentPattern([Windows.Automation.SelectionPattern]::Pattern, [ref]$selection) -or
      $null -eq $selection -or $selection.Current.CanSelectMultiple -isnot [bool] -or
      $selection.Current.CanSelectMultiple) {
    throw 'UNSUPPORTED_CONTROL|A single-selection dropdown is required'
  }
  return $selection
}
$null = Get-ChoiceSelectionPattern
function Test-ChoiceContainer($container) {
  # SelectionContainer may be the ComboBox or its nested List. Prove ancestry
  # by RuntimeId, never by a label, position or neighbouring controls.
  for ($depth = 0; $depth -le 12 -and $null -ne $container; $depth++) {
    $state = Get-UiaSafetyState $container
    if ($state.processId -ne $expectedProcessId -or $state.password -or $state.offscreen) { return $false }
    $id = (Get-UiaRuntimeId $container) -join ','
    if ($id -ceq $expectedRuntimeId) { return $true }
    if ($id -ceq ((Get-UiaRuntimeId $root) -join ',')) { return $false }
    $container = $walker.GetParent($container)
  }
  return $false
}
function Get-ChoiceItemPattern($item) {
  $state = Get-UiaSafetyState $item
  if ($state.processId -ne $expectedProcessId -or $state.password -or $state.offscreen -or
      -not $state.enabled -or $state.current.ControlType.ProgrammaticName -cne 'ControlType.ListItem') { return $null }
  $itemPattern = $null
  if (-not $item.TryGetCurrentPattern([Windows.Automation.SelectionItemPattern]::Pattern, [ref]$itemPattern) -or
      $null -eq $itemPattern -or -not (Test-ChoiceContainer $itemPattern.Current.SelectionContainer)) { return $null }
  $name = $state.current.Name
  if ($null -eq $name -or -not [string]::Equals([string]$name, $optionLabel, [StringComparison]::Ordinal)) { return $null }
  if ($itemPattern.Current.IsSelected -isnot [bool]) { throw 'TARGET_UNAVAILABLE|The option state is unavailable' }
  return $itemPattern
}
function Find-VisibleChoice {
  $stack = [Collections.Generic.Stack[object]]::new()
  $first = $walker.GetFirstChild($target)
  if ($null -ne $first) { $stack.Push(@{ element = $first; depth = 1 }) }
  $found = $null
  while ($stack.Count -gt 0) {
    Use-UiaNodeBudget
    $entry = $stack.Pop()
    $item = $entry.element
    $next = $walker.GetNextSibling($item)
    if ($null -ne $next) { $stack.Push(@{ element = $next; depth = $entry.depth }) }
    $state = Get-UiaSafetyState $item
    if ($state.password -or $state.offscreen -or $state.processId -ne $expectedProcessId) { continue }
    if ($null -ne (Get-ChoiceItemPattern $item)) {
      if ($null -ne $found) { throw 'AMBIGUOUS_TARGET|More than one visible option matches this dropdown' }
      $found = $item
    }
    $child = $walker.GetFirstChild($item)
    if ($null -ne $child) {
      if ($entry.depth -ge 12) { throw 'SCAN_LIMIT|The visible option tree is too deep' }
      $stack.Push(@{ element = $child; depth = $entry.depth + 1 })
    }
  }
  return $found
}
function Test-ExactSelection([string]$optionId) {
  $selection = Get-ChoiceSelectionPattern
  $items = @($selection.Current.GetSelection())
  # Only compare the identity of the previously visible, verified option.
  # Do not read a hidden option's Name, Value or Text after selection closes it.
  return $items.Count -eq 1 -and ((Get-UiaRuntimeId $items[0]) -join ',') -ceq $optionId
}
try {
  # Once navigation starts, any subsequent failure is conservatively unknown.
  # Never retry an effect or run a cleanup mutation against a changed target.
  if ($before -ceq 'Collapsed') { $comboPattern.Expand() }
  $choice = $null
  for ($read = 0; $read -lt 11; $read++) {
    $comboPattern = Assert-UiaControlTarget $target
    if ((Get-UiaControlOperationState $comboPattern) -cne 'Expanded') { throw 'Dropdown did not remain expanded' }
    $choice = Find-VisibleChoice
    if ($null -ne $choice) { break }
    if ($read -lt 10) { Start-Sleep -Milliseconds 25 }
  }
  if ($null -eq $choice) { throw 'The exact visible option is unavailable' }
  $optionId = (Get-UiaRuntimeId $choice) -join ','
  $null = Assert-UiaControlTarget $target
  $itemPattern = Get-ChoiceItemPattern $choice
  if ($null -eq $itemPattern) { throw 'The selected option changed before dispatch' }
  if (-not $itemPattern.Current.IsSelected) { $itemPattern.Select() }
  $matched = $false
  for ($read = 0; $read -lt 11; $read++) {
    $matched = Test-ExactSelection $optionId
    if ($matched) { break }
    if ($read -lt 10) { Start-Sleep -Milliseconds 25 }
  }
  if (-not $matched) { throw 'Selection could not be verified' }
  $comboPattern = Assert-UiaControlTarget $target
  $expanded = Get-UiaControlOperationState $comboPattern
  if ($expanded -ceq 'Expanded') { $comboPattern.Collapse() }
  elseif ($expanded -cne 'Collapsed') { throw 'Dropdown state changed before collapse' }
  $matched = $false
  for ($read = 0; $read -lt 11; $read++) {
    $comboPattern = Assert-UiaControlTarget $target
    $matched = (Get-UiaControlOperationState $comboPattern) -ceq 'Collapsed' -and (Test-ExactSelection $optionId)
    if ($matched) { break }
    if ($read -lt 10) { Start-Sleep -Milliseconds 25 }
  }
  if (-not $matched) { throw 'Final selection and collapsed state could not be verified' }
  $json = @{ status = 'verified'; action = 'choose' } | ConvertTo-Json -Compress
  $bytes = [Text.Encoding]::UTF8.GetBytes($json)
  $stdout = [Console]::OpenStandardOutput()
  $stdout.Write($bytes, 0, $bytes.Length)
  $stdout.Flush()
} catch { throw 'RESULT_UNKNOWN|The dropdown may have opened or its selection changed; inspect before retrying' }
