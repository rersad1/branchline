# Session-only bootstrap. Never writes profiles, history, or project state.
$global:__BranchlineUserPrompt = $function:prompt
$global:__BranchlineDefaultPrompt = $function:prompt.ToString()
try {
    Import-Module PSReadLine -ErrorAction Stop
    # Explicit token colors in this session; the user's profiles may override them.
    Set-PSReadLineOption -Colors @{
        Command = 'Blue'; Keyword = 'Magenta'; String = 'Green'; Comment = 'DarkGray'
        Parameter = 'Cyan'; Operator = 'DarkCyan'; Number = 'Magenta'; Type = 'Cyan'
        Variable = 'DarkCyan'; Member = 'Blue'; Error = 'Red'
    }
    Set-PSReadLineKeyHandler -Key Ctrl+d -Function DeleteCharOrExit
    # PSReadLine 2.0 decodes ASCII controls using the console's starting layout
    # (upstream #1393). Add native key-handler aliases for non-ASCII letters in
    # already loaded layouts. No keyboard layout or desktop settings are changed.
    if ($PSVersionTable.PSEdition -eq 'Desktop' -and (Get-Module PSReadLine).Version -lt [version]'2.4') {
        Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class BranchlineConsoleKeys {
    [DllImport("user32.dll")] static extern int GetKeyboardLayoutList(int count, [Out] IntPtr[] layouts);
    [DllImport("user32.dll")] static extern uint MapVirtualKeyEx(uint key, uint kind, IntPtr layout);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int ToUnicodeEx(uint key, uint scan, byte[] state, StringBuilder chars, int count, uint flags, IntPtr layout);
    public static string[] Aliases() {
        var layouts = new IntPtr[GetKeyboardLayoutList(0, null)];
        GetKeyboardLayoutList(layouts.Length, layouts);
        var result = new List<string>();
        foreach (var layout in layouts) for (uint key=65; key<=90; key++) {
            var chars = new StringBuilder(8);
            // Flag 4: do not alter keyboard/dead-key state (Windows 10+).
            if (ToUnicodeEx(key, MapVirtualKeyEx(key, 0, layout), new byte[256], chars, 8, 4, layout)==1
                && chars.Length==1 && chars[0]>127 && char.IsLetter(chars[0]))
                result.Add(((char)(key+32)).ToString()+"="+chars[0]);
        }
        return result.ToArray();
    }
}
'@
        $handlers = @(Get-PSReadLineKeyHandler)
        foreach ($alias in [BranchlineConsoleKeys]::Aliases()) {
            $parts = $alias.Split('=')
            $handler = $handlers | Where-Object { $_.Key -ceq ('Ctrl+' + $parts[0]) } | Select-Object -First 1
            if ($handler) {
                Set-PSReadLineKeyHandler -Chord ('Ctrl+' + $parts[1]) -Function $handler.Function
            }
        }
    }
    $option = Get-Command Set-PSReadLineOption
    if ($option.Parameters.ContainsKey('PredictionSource')) {
        Set-PSReadLineOption -PredictionSource History
    }
} catch { Write-Warning 'Branchline: some PSReadLine enhancements unavailable; using shell defaults.' }
# Match the normal Windows PowerShell profile order; custom bindings win.
foreach ($file in @($PROFILE.AllUsersAllHosts, $PROFILE.AllUsersCurrentHost, $PROFILE.CurrentUserAllHosts, $PROFILE.CurrentUserCurrentHost)) {
    if (Test-Path -LiteralPath $file) { try { . $file } catch { Write-Warning "Profile: $_" } }
}
if ($env:BRANCHLINE_SHELL_INTEGRATION -eq '0') { return }
$global:__BranchlineUserPrompt = $function:prompt
$global:__BranchlineNativePaste = $false
try { if (Get-Module PSReadLine) { $global:__BranchlineNativePaste = @((Get-PSReadLineKeyHandler) | Where-Object { $_.Key -ceq 'Ctrl+v' -and $_.Function -eq 'Paste' }).Count -gt 0 } } catch {}
$global:__BranchlineReadLine = $function:PSConsoleHostReadLine
if ($global:__BranchlineNativePaste -and $global:__BranchlineReadLine) {
    function global:PSConsoleHostReadLine {
        $line = & $global:__BranchlineReadLine
        # Foreground commands/TUIs receive their own paste protocol and keys.
        [Console]::Write("$([char]27)]633;P;NativePaste=0$([char]7)")
        $line
    }
}
$global:__BranchlineCompact = ($env:BRANCHLINE_PROMPT -eq 'compact' -or
    ($env:BRANCHLINE_PROMPT -ne 'user' -and $function:prompt.ToString() -eq $global:__BranchlineDefaultPrompt))
function global:branchline_prompt([ValidateSet('compact','user')]$Mode) {
    $global:__BranchlineCompact = $Mode -eq 'compact'
}
function global:branchline_integration_off {
    $function:global:prompt = $global:__BranchlineUserPrompt
    if ($global:__BranchlineReadLine) { $function:global:PSConsoleHostReadLine = $global:__BranchlineReadLine }
    [Console]::Write("$([char]27)]633;P;Cwd=$([char]7)$([char]27)]633;P;NativePaste=0$([char]7)")
}
function global:prompt {
    $ok = $?
    $last = $global:LASTEXITCODE
    $location = $ExecutionContext.SessionState.Path.CurrentLocation
    $path = ''
    if ($location.Provider.Name -eq 'FileSystem') { $path = $location.ProviderPath }
    # URI escaping prevents OSC delimiters, Unicode and spaces from becoming syntax.
    [Console]::Write("$([char]27)]633;P;Cwd=$([Uri]::EscapeDataString($path))$([char]7)")
    if ($global:__BranchlineNativePaste) { [Console]::Write("$([char]27)]633;P;NativePaste=1$([char]7)") }
    if (-not $global:__BranchlineCompact) {
        # Preserve $? for a user's status-aware prompt (no output/error record).
        if (-not $ok) { Write-Error 'BranchlineStatus' -ErrorAction Ignore }
        $global:__BranchlineUserPrompt.Invoke()
    } else {
        $leaf = Split-Path -Leaf $location.Path
        if (-not $leaf) { $leaf = $location.Path }
        # Do not allow control characters in a directory name to repaint the screen.
        $leaf = $leaf -replace '[\x00-\x1f\x7f-\x9f]', '?'
        if ($leaf.Length -gt 40) { $leaf = '…' + $leaf.Substring($leaf.Length - 39) }
        $failure = ''
        if (-not $ok) { $failure = ' !' }
        "$([char]27)[34m$leaf$([char]27)[31m$failure$([char]27)[0m > "
    }
    $global:LASTEXITCODE = $last
}
