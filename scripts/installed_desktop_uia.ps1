param(
    [Parameter(Mandatory)][int]$TargetPid,
    [Parameter(Mandatory)][string]$ExpectedExecutable,
    [Parameter(Mandatory)][ValidateSet('Ready', 'Send', 'Reply')][string]$Operation,
    [Parameter(Mandatory)][string]$DailyHeading,
    [Parameter(Mandatory)][string]$ComposerName,
    [Parameter(Mandatory)][string]$SendName,
    [string]$PromptText = '',
    [string]$ReplyText = ''
)

$ErrorActionPreference = 'Stop'
$evidence = [ordered]@{
    processAlive = $false
    windowFound = $false
    dailyHeadingFound = $false
    composerFound = $false
    composerFocused = $false
    valuePatternUsed = $false
    inputMatched = $false
    sendEnabled = $false
    sendInvoked = $false
    replyFound = $false
}
$success = $false

function Wait-ForElement([scriptblock]$Probe) {
    $deadline = [DateTime]::UtcNow.AddSeconds(25)
    do {
        try {
            $result = & $Probe
            if ($null -ne $result) { return $result }
        } catch [System.Windows.Automation.ElementNotAvailableException] {
            # WebView2 may replace accessibility nodes during a render.
        }
        Start-Sleep -Milliseconds 200
    } until ([DateTime]::UtcNow -ge $deadline)
    throw 'Timed out waiting for the expected installed-app control.'
}

function Find-Control($Root, [string]$Name, $Type = $null) {
    $condition = [System.Windows.Automation.PropertyCondition]::new(
        [System.Windows.Automation.AutomationElement]::NameProperty, $Name
    )
    if ($null -ne $Type) {
        $typeCondition = [System.Windows.Automation.PropertyCondition]::new(
            [System.Windows.Automation.AutomationElement]::ControlTypeProperty, $Type
        )
        $condition = [System.Windows.Automation.AndCondition]::new($condition, $typeCondition)
    }
    $results = $Root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $condition)
    foreach ($result in $results) {
        if (-not $result.Current.IsOffscreen) { return $result }
    }
    return $null
}

try {
    if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted' -or
        $env:RUNNER_OS -ne 'Windows') {
        throw 'This control probe requires a disposable GitHub-hosted Windows runner.'
    }
    Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Windows.Forms
    $process = Get-Process -Id $TargetPid
    if (-not [StringComparer]::OrdinalIgnoreCase.Equals($process.Path, $ExpectedExecutable)) {
        throw 'Automation target does not match the installed executable.'
    }
    $root = Wait-ForElement {
        $process.Refresh()
        if ($process.HasExited) { throw 'The installed process exited.' }
        $evidence.processAlive = $true
        $handle = $process.MainWindowHandle
        if ($handle -eq [IntPtr]::Zero) { return $null }
        $element = [System.Windows.Automation.AutomationElement]::FromHandle($handle)
        if ($element.Current.ProcessId -ne $TargetPid) { throw 'Window belongs to another process.' }
        $evidence.windowFound = $true
        return $element
    }
    $edit = [System.Windows.Automation.ControlType]::Edit
    $button = [System.Windows.Automation.ControlType]::Button
    $null = Wait-ForElement {
        $heading = Find-Control $root $DailyHeading
        if ($null -ne $heading) { $evidence.dailyHeadingFound = $true }
        return $heading
    }
    $composer = Wait-ForElement {
        $candidate = Find-Control $root $ComposerName $edit
        if ($null -ne $candidate -and $candidate.Current.IsEnabled) {
            $evidence.composerFound = $true
            return $candidate
        }
        return $null
    }

    if ($Operation -eq 'Send') {
        if ($PromptText -notmatch '^[A-Za-z0-9 :.,-]+$') { throw 'Unexpected synthetic prompt.' }
        $valuePattern = $null
        if ($composer.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern, [ref]$valuePattern)) {
            if ($valuePattern.Current.IsReadOnly) { throw 'The owned composer is read-only.' }
            $valuePattern.SetValue($PromptText)
            $evidence.valuePatternUsed = $true
        } else {
            Add-Type @'
using System;
using System.Runtime.InteropServices;

public static class InstalledSmokeFocus
{
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hwnd);
}
'@
            [void][InstalledSmokeFocus]::SetForegroundWindow($process.MainWindowHandle)
            $composer.SetFocus()
            $process.Refresh()
            if ([InstalledSmokeFocus]::GetForegroundWindow() -ne $process.MainWindowHandle -or
                -not $composer.Current.HasKeyboardFocus) {
                throw 'The owned composer could not acquire keyboard focus.'
            }
            $evidence.composerFocused = $true
            # Multiline controls may expose only TextPattern. Type only after
            # checking the focused element and its owned foreground HWND.
            [System.Windows.Forms.SendKeys]::SendWait($PromptText)
        }
        $send = Wait-ForElement {
            $input = Find-Control $root $ComposerName $edit
            if ($null -eq $input) { return $null }
            $pattern = $null
            $value = $null
            if ($input.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern, [ref]$pattern)) {
                $value = $pattern.Current.Value
            } elseif ($input.TryGetCurrentPattern([System.Windows.Automation.TextPattern]::Pattern, [ref]$pattern)) {
                $value = $pattern.DocumentRange.GetText(-1).TrimEnd([char[]]@([char]13, [char]10))
            }
            if ($value -ne $PromptText) { return $null }
            $evidence.inputMatched = $true
            $candidate = Find-Control $root $SendName $button
            if ($null -ne $candidate -and $candidate.Current.IsEnabled) {
                $evidence.sendEnabled = $true
                return $candidate
            }
            return $null
        }
        $invoke = $null
        if (-not $send.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$invoke)) {
            throw 'The visible send button has no supported invoke action.'
        }
        $invoke.Invoke()
        $evidence.sendInvoked = $true
    }

    if ($Operation -eq 'Reply') {
        if (-not $ReplyText) { throw 'Expected synthetic reply is missing.' }
        $null = Wait-ForElement {
            $reply = Find-Control $root $ReplyText
            if ($null -ne $reply) { $evidence.replyFound = $true }
            return $reply
        }
    }
    $success = $true
} catch {
    # Publish only the bounded milestones. UIA failures can contain window text.
} finally {
    $evidence | ConvertTo-Json -Compress
}
if (-not $success) { exit 1 }
