$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public sealed class AngelBotControlWindowCandidate {
  public readonly long Handle;
  public readonly uint ProcessId;
  public readonly int ExtendedStyle;
  public readonly bool StyleKnown;
  public AngelBotControlWindowCandidate(long handle, uint processId, int extendedStyle, bool styleKnown) {
    Handle = handle; ProcessId = processId; ExtendedStyle = extendedStyle; StyleKnown = styleKnown;
  }
}
public static class AngelBotCaptureWindows {
  private delegate bool EnumCallback(IntPtr hwnd, IntPtr param);
  [DllImport("user32.dll")] private static extern bool EnumWindows(EnumCallback callback, IntPtr param);
  [DllImport("user32.dll")] private static extern bool EnumChildWindows(IntPtr parent, EnumCallback callback, IntPtr param);
  [DllImport("user32.dll")] private static extern bool IsWindowVisible(IntPtr hwnd);
  [DllImport("user32.dll")] private static extern IntPtr GetWindow(IntPtr hwnd, uint command);
  [DllImport("user32.dll")] private static extern IntPtr GetParent(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint processId);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hwnd);
  [DllImport("kernel32.dll", EntryPoint = "SetLastError", SetLastError = true)] private static extern void ResetLastError(uint error);
  [DllImport("user32.dll", EntryPoint = "GetWindowLongW", SetLastError = true)] private static extern int GetWindowLong(IntPtr hwnd, int index);
  private static bool IsAuxiliary(AngelBotControlWindowCandidate candidate) {
    const int toolWindow = 0x00000080;
    const int noActivate = 0x08000000;
    const int appWindow = 0x00040000;
    return candidate.StyleKnown &&
      (candidate.ExtendedStyle & (toolWindow | noActivate)) == (toolWindow | noActivate) &&
      (candidate.ExtendedStyle & appWindow) == 0;
  }
  public static long[] SelectControlRoots(AngelBotControlWindowCandidate[] candidates) {
    // A popup-like surface is not a second primary root when its own process
    // has a known primary window. Never collapse genuine windows, unknown
    // metadata, or different instances into an arbitrary first candidate.
    var primaryProcesses = new HashSet<uint>();
    foreach (var candidate in candidates) {
      if (candidate.ProcessId != 0 && candidate.StyleKnown && !IsAuxiliary(candidate))
        primaryProcesses.Add(candidate.ProcessId);
    }
    var handles = new List<long>();
    foreach (var candidate in candidates) {
      if (!IsAuxiliary(candidate) || !primaryProcesses.Contains(candidate.ProcessId))
        handles.Add(candidate.Handle);
    }
    return handles.ToArray();
  }
  public static long[] EligibleWindows() {
    var result = new List<long>();
    if (!EnumWindows((hwnd, unused) => {
      if (IsWindowVisible(hwnd) && GetWindow(hwnd, 4) == IntPtr.Zero) result.Add(hwnd.ToInt64());
      return true;
    }, IntPtr.Zero)) throw new InvalidOperationException();
    return result.ToArray();
  }
  public static long[] EligibleControlWindows(uint[] processIds) {
    var trusted = new HashSet<uint>(processIds);
    var result = new List<long>();
    foreach (long handle in EligibleWindows()) {
      var top = new IntPtr(handle);
      uint processId;
      GetWindowThreadProcessId(top, out processId);
      if (trusted.Contains(processId)) { result.Add(handle); continue; }
      // Packaged apps can own a CoreWindow inside a separate Windows host.
      // Keep only the outermost HWND owned by that exact trusted process;
      // never return the host's root or authorize its other children.
      EnumChildWindows(top, (child, unused) => {
        uint childId;
        if (!IsWindowVisible(child) || GetWindowThreadProcessId(child, out childId) == 0 || !trusted.Contains(childId)) return true;
        var parent = GetParent(child);
        bool reachedTop = false;
        for (int depth = 0; parent != IntPtr.Zero; depth++) {
          if (depth >= 64) throw new InvalidOperationException("window ancestry limit");
          uint parentId;
          if (GetWindowThreadProcessId(parent, out parentId) == 0) return true;
          if (parentId == childId) return true;
          if (parent == top) { reachedTop = true; break; }
          parent = GetParent(parent);
        }
        if (reachedTop) result.Add(child.ToInt64());
        return true;
      }, IntPtr.Zero);
    }
    var candidates = new List<AngelBotControlWindowCandidate>();
    foreach (long handle in result) {
      var window = new IntPtr(handle);
      uint processId;
      GetWindowThreadProcessId(window, out processId);
      ResetLastError(0);
      int style = GetWindowLong(window, -20);
      bool known = style != 0 || Marshal.GetLastWin32Error() == 0;
      candidates.Add(new AngelBotControlWindowCandidate(handle, processId, style, known));
    }
    return SelectControlRoots(candidates.ToArray());
  }
}
'@
function Get-UiaControlWindows([string]$expected) {
  $processIds = New-Object 'System.Collections.Generic.List[uint32]'
  Get-Process | ForEach-Object {
    try {
      if ([IO.Path]::GetFullPath($_.Path) -ieq $expected) { $processIds.Add([uint32]$_.Id) }
    } catch {}
  }
  if ($processIds.Count -gt 0) {
    [AngelBotCaptureWindows]::EligibleControlWindows($processIds.ToArray()) | ForEach-Object { [IntPtr]::new([long]$_) }
  }
}
function Get-UiaWindowProcessId([IntPtr]$handle) {
  $processId = [uint32]0
  if ([AngelBotCaptureWindows]::GetWindowThreadProcessId($handle, [ref]$processId) -eq 0 -or $processId -eq 0) {
    throw 'TARGET_CHANGED|The selected window no longer has a valid process owner'
  }
  return $processId
}
