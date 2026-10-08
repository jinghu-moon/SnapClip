# A one-off launcher for the P0.09 UIPI arm: start a process at LOW integrity so that it can
# try to inject input into a HIGH integrity window. Evidence and results: docs/30 §24.6.2,
# task and exit conditions: docs/31 §6 (P0.09).
#
# `runas /trustlevel:0x20000` does not do this on this machine: it produced a token whose
# mandatory label is still High (S-1-16-12288), because the restricted token inherits the
# integrity level of the (High) parent. Lowering the level needs the token itself to be
# rewritten, which is what this script does.
#
# Usage (the arm itself is `cargo test -p snapclip-capture --lib uipi_probe -- --ignored`):
#
#   cargo test -p snapclip-capture --lib --no-run     # note the printed executable path
#   $env:SNAPCLIP_UIPI_TARGET = 'some window title'   # a scrollable High window
#   pwsh -File tools/p009-low-integrity-launch.ps1 `
#     -CommandLine 'cmd /c "<test exe>" uipi_probe --ignored --nocapture --test-threads=1 > <log>' `
#     -CreationFlags 0x08000000
#
# The child writes its log into a directory whose mandatory label was lowered
# (`icacls <dir> /setintegritylevel (OI)(CI)L`); a normal user directory is "no write up" and
# silently drops the child's redirected output.
param(
    [Parameter(Mandatory = $true)][string]$CommandLine,
    [string]$IntegritySid = 'S-1-16-4096',   # Low
    # 0x08000000 = CREATE_NO_WINDOW. Needed for the injection arm: a low-integrity child cannot
    # attach to a high-integrity console, so Windows allocates one, and the console window lands
    # near the target's centre — where the probe aims its wheel. Without this flag the wheel goes
    # to the console and the arm reports a movement it never measured.
    [int]$CreationFlags = 0
)

$ErrorActionPreference = 'Stop'

Add-Type -Namespace P009 -Name Low -MemberDefinition @'
[StructLayout(LayoutKind.Sequential)]
public struct STARTUPINFO {
    public int cb;
    public IntPtr lpReserved, lpDesktop, lpTitle;
    public int dwX, dwY, dwXSize, dwYSize, dwXCountChars, dwYCountChars, dwFillAttribute, dwFlags;
    public short wShowWindow, cbReserved2;
    public IntPtr lpReserved2, hStdInput, hStdOutput, hStdError;
}
[StructLayout(LayoutKind.Sequential)]
public struct PROCESS_INFORMATION {
    public IntPtr hProcess, hThread;
    public int dwProcessId, dwThreadId;
}
[StructLayout(LayoutKind.Sequential)]
public struct SID_AND_ATTRIBUTES {
    public IntPtr Sid;
    public int Attributes;
}
[StructLayout(LayoutKind.Sequential)]
public struct TOKEN_MANDATORY_LABEL {
    public SID_AND_ATTRIBUTES Label;
}

[DllImport("kernel32.dll")] public static extern IntPtr GetCurrentProcess();
[DllImport("kernel32.dll", SetLastError = true)] public static extern bool CloseHandle(IntPtr h);
[DllImport("advapi32.dll", SetLastError = true)]
public static extern bool OpenProcessToken(IntPtr process, int access, out IntPtr token);
[DllImport("advapi32.dll", SetLastError = true)]
public static extern bool DuplicateTokenEx(IntPtr token, int access, IntPtr attrs, int level, int type, out IntPtr newToken);
[DllImport("advapi32.dll", SetLastError = true)]
public static extern bool SetTokenInformation(IntPtr token, int infoClass, IntPtr info, int infoLength);
[DllImport("advapi32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
public static extern bool ConvertStringSidToSidW(string sid, out IntPtr sidPtr);
[DllImport("advapi32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
public static extern bool CreateProcessWithTokenW(IntPtr token, int logonFlags, string app, string cmd,
    int creationFlags, IntPtr env, string cwd, ref STARTUPINFO si, out PROCESS_INFORMATION pi);

public static string Launch(string commandLine, string integritySid, int creationFlags) {
    const int TOKEN_ALL_ACCESS = 0x000F01FF;
    const int SecurityImpersonation = 2;
    const int TokenPrimary = 1;
    const int TokenIntegrityLevel = 25;
    const int SE_GROUP_INTEGRITY = 0x00000020;

    IntPtr processToken;
    if (!OpenProcessToken(GetCurrentProcess(), TOKEN_ALL_ACCESS, out processToken))
        return "OpenProcessToken failed: " + Marshal.GetLastWin32Error();
    IntPtr duplicate;
    if (!DuplicateTokenEx(processToken, TOKEN_ALL_ACCESS, IntPtr.Zero, SecurityImpersonation, TokenPrimary, out duplicate))
        return "DuplicateTokenEx failed: " + Marshal.GetLastWin32Error();
    IntPtr sidPtr;
    if (!ConvertStringSidToSidW(integritySid, out sidPtr))
        return "ConvertStringSidToSid failed: " + Marshal.GetLastWin32Error();

    var label = new TOKEN_MANDATORY_LABEL();
    label.Label.Sid = sidPtr;
    label.Label.Attributes = SE_GROUP_INTEGRITY;
    int labelSize = Marshal.SizeOf(typeof(TOKEN_MANDATORY_LABEL));
    int sidLength = GetLengthSid(sidPtr);
    IntPtr buffer = Marshal.AllocHGlobal(labelSize + sidLength);
    Marshal.StructureToPtr(label, buffer, false);
    if (!SetTokenInformation(duplicate, TokenIntegrityLevel, buffer, labelSize + sidLength))
        return "SetTokenInformation failed: " + Marshal.GetLastWin32Error();

    var si = new STARTUPINFO();
    si.cb = Marshal.SizeOf(typeof(STARTUPINFO));
    PROCESS_INFORMATION pi;
    if (!CreateProcessWithTokenW(duplicate, 0, null, commandLine, creationFlags, IntPtr.Zero, null, ref si, out pi))
        return "CreateProcessWithTokenW failed: " + Marshal.GetLastWin32Error();
    CloseHandle(pi.hProcess);
    CloseHandle(pi.hThread);
    return "started pid " + pi.dwProcessId;
}

[DllImport("advapi32.dll")] public static extern int GetLengthSid(IntPtr sid);
'@ -ReferencedAssemblies System.Runtime.InteropServices

$result = [P009.Low]::Launch($CommandLine, $IntegritySid, $CreationFlags)
Write-Host $result
