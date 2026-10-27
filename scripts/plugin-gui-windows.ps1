param(
    [Parameter(Mandatory = $true)][string]$Binary,
    [Parameter(Mandatory = $true)][string]$TestHome,
    [Parameter(Mandatory = $true)][string]$Report
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Windows.Forms, System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class GuiNative {
    public delegate bool EnumCallback(IntPtr hwnd, IntPtr lParam);
    [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extraInfo);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCallback callback, IntPtr data);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out Rect rect);
    public static IntPtr MainWindow(uint processId) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((hwnd, ignored) => {
            uint pid;
            Rect rect;
            GetWindowThreadProcessId(hwnd, out pid);
            if (pid == processId && IsWindowVisible(hwnd) && GetWindowRect(hwnd, out rect)
                && rect.Right - rect.Left >= 800 && rect.Bottom - rect.Top >= 500) {
                found = hwnd;
                return false;
            }
            return true;
        }, IntPtr.Zero);
        return found;
    }
}
'@
[void][GuiNative]::SetProcessDPIAware()
$env:CC_SWITCH_TEST_HOME = $TestHome
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--force-renderer-accessibility'
$evidence = [ordered]@{ platform = 'windows'; success = $false; steps = @() }
$main = $null
$pluginPid = $null
$window = [IntPtr]::Zero
$reportDir = Split-Path -Parent $Report
New-Item -ItemType Directory -Force -Path $reportDir | Out-Null

function Wait-Until([scriptblock]$Condition, [string]$Description, [int]$Seconds = 45) {
    $deadline = (Get-Date).AddSeconds($Seconds)
    do {
        $value = & $Condition
        if ($value) { return $value }
        Start-Sleep -Milliseconds 250
    } while ((Get-Date) -lt $deadline)
    throw "Timed out waiting for $Description"
}

function Find-Element([string]$Name, [string]$Type = '') {
    if ($window -eq [IntPtr]::Zero -or -not [GuiNative]::IsWindowVisible($window)) { return $null }
    $root = [System.Windows.Automation.AutomationElement]::FromHandle($window)
    if ($null -eq $root) { return $null }
    $condition = New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::NameProperty, $Name)
    $matches = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $condition)
    foreach ($element in $matches) {
        $state = $element.Current
        if ($state.IsOffscreen -or $state.BoundingRectangle.IsEmpty) { continue }
        if ($Type -and $state.ControlType.ProgrammaticName -ne "ControlType.$Type") { continue }
        return $element
    }
    return $null
}

function Click-Element([string]$Name, [string]$Type = 'Button') {
    [void][GuiNative]::SetForegroundWindow($window)
    Start-Sleep -Milliseconds 300
    $element = Wait-Until { Find-Element $Name $Type } "visible $Type '$Name'"
    $rect = $element.Current.BoundingRectangle
    [void][GuiNative]::SetCursorPos([int]($rect.X + $rect.Width / 2), [int]($rect.Y + $rect.Height / 2))
    [GuiNative]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)
    [GuiNative]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
}

function Screenshot([string]$Name) {
    $bounds = [System.Windows.Forms.SystemInformation]::VirtualScreen
    $image = New-Object System.Drawing.Bitmap($bounds.Width, $bounds.Height)
    $graphics = [System.Drawing.Graphics]::FromImage($image)
    try {
        $graphics.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size)
        $image.Save((Join-Path $reportDir "$Name.png"), [System.Drawing.Imaging.ImageFormat]::Png)
    } finally { $graphics.Dispose(); $image.Dispose() }
}

function Plugin-Process {
    $children = @(Get-CimInstance Win32_Process -Filter "ParentProcessId = $($main.Id)" |
        Where-Object { $_.CommandLine -like '*--cc-switch-plugin-runtime*' })
    if ($children.Count -eq 1) { return $children[0] }
    return $null
}

function Assert-PluginAlive {
    $child = Plugin-Process
    if (-not $child -or $child.ProcessId -ne $pluginPid) { throw 'Plugin process changed or exited' }
    $process = Get-Process -Id $pluginPid -ErrorAction Stop
    $process.Refresh()
    if ($process.MainWindowHandle -eq [IntPtr]::Zero -or -not [GuiNative]::IsWindowVisible($process.MainWindowHandle)) {
        throw 'Plugin native window is not visible'
    }
}

try {
    $main = Start-Process -FilePath $Binary -PassThru
    $window = Wait-Until {
        $main.Refresh()
        if ($main.HasExited) { throw "Main application exited with $($main.ExitCode)" }
        $handle = [GuiNative]::MainWindow($main.Id)
        if ($handle -ne [IntPtr]::Zero) { $handle }
    } 'main application window' 90
    $evidence.mainPid = $main.Id
    Click-Element 'Plugins'
    [void](Wait-Until { Find-Element 'GUI Acceptance' } 'installed plugin')
    Click-Element 'Open'
    [void](Wait-Until { (Find-Element 'Stop plugin' 'Button') -and (Find-Element 'Plugin is open in a separate window') } 'running plugin' 60)
    $child = Wait-Until { Plugin-Process } 'plugin host process'
    $pluginPid = $child.ProcessId
    $evidence.pluginPid = $pluginPid
    [void](Wait-Until {
        $p = Get-Process -Id $pluginPid -ErrorAction SilentlyContinue
        if ($p) { $p.Refresh(); if ($p.MainWindowHandle -ne [IntPtr]::Zero -and [GuiNative]::IsWindowVisible($p.MainWindowHandle)) { $true } }
    } 'visible plugin native window')
    $evidence.steps += 'opened_plugin'

    Click-Element 'Skills'
    [void](Wait-Until { Find-Element 'More Skills actions' } 'Skills page')
    Assert-PluginAlive
    $evidence.steps += 'switched_to_skills_same_plugin_window'
    Click-Element 'Plugins'
    [void](Wait-Until { (Find-Element 'Stop plugin' 'Button') -and (Find-Element 'Plugin is open in a separate window') } 'returned plugin status')
    Assert-PluginAlive
    Screenshot 'windows-returned'
    $evidence.steps += 'returned_to_plugin_same_window'

    Click-Element 'Close window'
    [void](Wait-Until { -not [GuiNative]::IsWindowVisible($window) } 'main window hidden to tray')
    Start-Sleep -Seconds 2
    $main.Refresh()
    if ($main.HasExited) { throw 'Main application exited instead of hiding to tray' }
    Assert-PluginAlive
    $evidence.steps += 'closed_main_to_tray_same_plugin_window'

    # A second launch uses the application's normal single-instance activation to restore the main window.
    $activation = Start-Process -FilePath $Binary -PassThru
    [void](Wait-Until { [GuiNative]::IsWindowVisible($window) } 'main window restored from tray' 60)
    [void](Wait-Until { (Find-Element 'Stop plugin' 'Button') -and (Find-Element 'Plugin is open in a separate window') } 'plugin status after restore')
    Assert-PluginAlive
    Click-Element 'Stop plugin'
    [void](Wait-Until { -not (Get-Process -Id $pluginPid -ErrorAction SilentlyContinue) } 'plugin process exit')
    $profiles = Join-Path $TestHome '.cc-switch/ui-plugin-profiles'
    [void](Wait-Until { -not (Test-Path $profiles) -or @(Get-ChildItem -Force $profiles).Count -eq 0 } 'plugin profile removal')
    $evidence.steps += 'restored_and_stopped_plugin_profile_removed'
    $evidence.success = $true
} catch {
    $evidence.error = $_.Exception.ToString()
    try {
        Copy-Item (Join-Path $TestHome '.cc-switch/ui-plugins/plugin-test.gui-acceptance.json') (Join-Path $reportDir 'windows-plugin-record.json')
        $logs = Join-Path $TestHome '.cc-switch/logs'
        if (Test-Path $logs) { Copy-Item $logs (Join-Path $reportDir 'windows-app-logs') -Recurse }
    } catch { $evidence.diagnosticError = $_.Exception.ToString() }
    try { Screenshot 'windows-failure' } catch { }
    try {
        $root = [System.Windows.Automation.AutomationElement]::FromHandle($window)
        $nodes = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
        $lines = @("Window=$window Root=$($root.Current.Name) Count=$($nodes.Count)")
        for ($i = 0; $i -lt $nodes.Count; $i++) {
            try {
                $node = $nodes.Item($i).Current
                $lines += "$($node.ControlType.ProgrammaticName)`t$($node.Name)`t$($node.IsOffscreen)`t$($node.BoundingRectangle)"
            } catch { $lines += "Node $i unavailable: $_" }
        }
        $lines | Set-Content -Encoding UTF8 (Join-Path $reportDir 'windows-uia-tree.txt')
    } catch { $_.ToString() | Set-Content -Encoding UTF8 (Join-Path $reportDir 'windows-uia-error.txt') }
} finally {
    $evidence | ConvertTo-Json -Depth 6 | Set-Content -Encoding UTF8 $Report
    # The runner is isolated. Do not leave a failed test's native processes behind.
    if ($pluginPid) { Stop-Process -Id $pluginPid -Force -ErrorAction SilentlyContinue }
    if ($main) { Stop-Process -Id $main.Id -Force -ErrorAction SilentlyContinue }
}
if (-not $evidence.success) { throw $evidence.error }
