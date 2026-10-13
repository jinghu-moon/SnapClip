# 给「贴图占多少常驻内存」量的一个读数工具。
#
# 用法：
#   # 先启动应用（或已经在跑），什么都不贴时读一次基线
#   powershell -NoProfile -File scripts\measure-pin-memory.ps1 -Label baseline
#   # 用户按 F5/F6 贴几张，再读一次
#   powershell -NoProfile -File scripts\measure-pin-memory.ps1 -Label 3pins
#   # 用双击全部关掉，再读一次
#   powershell -NoProfile -File scripts\measure-pin-memory.ps1 -Label after-close
#
# 读的是 `.NET Process` 的 `WorkingSet64` / `PrivateMemorySize64`，两个数一起看：
# 工作集包含可以换出的部分，私有字节才是这块进程真正占住的量。差值是判断
# 「一张 4K 贴图到底吃掉多少」的唯一依据——设计文档里写的估算不算数。
#
# 之所以是个脚本而不是单元测试：这件事要量的对象是**活着的窗口**和它的 DIB，
# 单元测试里既没有窗口也没有 DIB 段。
#
# 两个踩过的坑写在这里，免得下次再踩：
# 1. 不能在这个文件里用 here-string（`@'...'@`）：Windows PowerShell 5.1 的
#    解析器对它很挑，一旦断行风格或缩进差一点就报 "The string is missing the
#    terminator"，而且错误会指向文件后面的无关行。P/Invoke 声明用单行写。
# 2. 不能给变量起名 `$pid`：它是 PowerShell 的只读自动变量，赋值会直接报错。
# 3. 这个文件**必须**带 UTF-8 BOM。Windows PowerShell 5.1 对没有 BOM 的文件按
#    系统 ANSI 码页解码，文件里的中文注释就成了乱码字节，某个字节恰好截断了一个
#    字符串字面量，于是解析错误报在后面几十行的无关位置（第一次报在第 63 行的
#    Write-Output 上）。仓库里别的 .ps1 没有 BOM 也活着，因为它们是纯 ASCII——
#    与其说是惯例，不如说是运气。以后新增带中文的 .ps1，一律存成带 BOM 的 UTF-8。
#    补 BOM 的写法（会改变文件字节，所以用 File API 而不是 Set-Content）：
#    [System.IO.File]::WriteAllText($path, $text, [System.Text.UTF8Encoding]::new($true))

param(
    [string] $ProcessName = 'snapclip-app',
    [string] $Label = '',
    [string] $PinWindowClass = 'SnapClipPinWindow',
    [switch] $Details,
    # 只删台账：一轮测试开始时先清一次，差值就只在这一轮里有意义。
    [switch] $Reset
)

$ErrorActionPreference = 'Stop'

# 仓库根目录。这个脚本放在 scripts\ 下，所以根是它的上一级。
# （踩过：这里原写成 $repoRoot，那不是一个自动变量，只会在 Join-Path 处抛错或拼出相对路径）
$repoRoot = Split-Path -Parent $PSScriptRoot
$ledgerPath = Join-Path $repoRoot 'target\pin-memory-ledger.json'

# `-Reset` 只做一件事：把台账删掉，让下一行读数成为新的一轮的第一行。
# 它必须在**任何测量之前**返回。踩过两次：第一次放在 `$ledgerPath` 之前（读到 $null，
# Test-Path 抛参数校验错），第二次放在了测量之后（于是 `-Reset` 自己会先打印一行读数）。
if ($Reset) {
    if (Test-Path -LiteralPath $ledgerPath -PathType Leaf) {
        Remove-Item -LiteralPath $ledgerPath -Force
        Write-Output "[pin-memory] ledger cleared: $ledgerPath"
    } else {
        Write-Output "[pin-memory] nothing to clear at $ledgerPath"
    }
    exit 0
}

# 两个读数之差，带正负号，保留一位小数。拿不到的值回 'n/a'。
# 用函数而不是 `$delta = { param(...) ... }` 脚本块：脚本块那种写法在
# Windows PowerShell 5.1 的解析器下直接报语法错（"Array index expression is missing"、
# "The string is missing the terminator"），而这个脚本的目标宿主就是 5.1。
function Format-Delta {
    param($Before, $After)
    if ($null -eq $Before -or $null -eq $After -or $Before -eq 'unavailable' -or $After -eq 'unavailable') {
        return 'n/a'
    }
    $difference = [double]$After - [double]$Before
    $sign = if ($difference -gt 0) { '+' } else { '' }
    return "$sign$([math]::Round($difference, 1))"
}

# 贴图窗口的类名，抄自 apps/snapclip/src/pin/host.rs 的 `PIN_WINDOW_CLASS`。
# 数窗口比数内存重要：内存涨了但没有窗口，和窗口还在没销毁，是两个不同的故障，
# 而「GDI 对象数」在两者上都可能给出同一个读数。
$pinWindowCount = 'unavailable'
try {
    $windowSignature = '[DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Unicode)] public static extern IntPtr FindWindowExW(IntPtr hWndParent, IntPtr hWndChildAfter, string lpszClass, string lpszWindow); [DllImport("user32.dll")] public static extern IntPtr GetDesktopWindow();'
    $windowType = 'SnapClipPinMemory.WindowFinder' -as [type]
    if (-not $windowType) {
        Add-Type -Namespace 'SnapClipPinMemory' -Name 'WindowFinder' -MemberDefinition $windowSignature
        $windowType = 'SnapClipPinMemory.WindowFinder' -as [type]
    }
    $desktop = [SnapClipPinMemory.WindowFinder]::GetDesktopWindow()
    $count = 0
    $previous = [IntPtr]::Zero
    while ($true) {
        $foundWindow = [SnapClipPinMemory.WindowFinder]::FindWindowExW($desktop, $previous, $PinWindowClass, $null)
        if ($foundWindow -eq [IntPtr]::Zero) { break }
        $count++
        $previous = $foundWindow
    }
    $pinWindowCount = [string]$count
} catch {
    $pinWindowCount = 'unavailable'
}

$found = Get-Process -Name $ProcessName -ErrorAction SilentlyContinue
if (-not $found) {
    Write-Error "没有找到正在运行的 $ProcessName。先启动 target\release\snapclip-app.exe。"
    exit 1
}
$process = $found | Sort-Object StartTime -Descending | Select-Object -First 1

$workingSetMib = [math]::Round($process.WorkingSet64 / 1MB, 1)
$privateMib = [math]::Round($process.PrivateMemorySize64 / 1MB, 1)
$processId = $process.Id
$labelText = if ($Label) { $Label } else { 'current' }

# GDI / USER 对象数需要 P/Invoke；拿不到就只报内存，不让整条命令失败。
$gdiText = 'unavailable'
try {
    $gdiType = 'SnapClipPinMemory.NativeResources' -as [type]
    if (-not $gdiType) {
        $signature = '[DllImport("user32.dll", SetLastError = true)] public static extern uint GetGuiResources(IntPtr hProcess, uint uiFlags);'
        Add-Type -Namespace 'SnapClipPinMemory' -Name 'NativeResources' -MemberDefinition $signature
        $gdiType = 'SnapClipPinMemory.NativeResources' -as [type]
    }
    $gdiText = [string][SnapClipPinMemory.NativeResources]::GetGuiResources($process.Handle, 0)
} catch {
    $gdiText = 'unavailable'
}

Write-Output "[pin-memory] label=$labelText pid=$processId pin_windows=$pinWindowCount working_set_mib=$workingSetMib private_mib=$privateMib gdi_objects=$gdiText"

# 和上一次读数比。这一块存在的理由是踩过的一个坑：上一次我请用户手写 -Label，
# 而三行读数里有一行写着 `baseline-0pins` 却量到 4 张贴图——标签是**人写的**，
# 窗口数是我**数出来的**，两者冲突时只有后者算数。差值必须由工具自己算，
# 否则我又会拿错的两个读数相减，得出一个看起来很像结论的错数。
# 台账放在 target\ 下：它是一次测量的产物，不是源文件。
$priorReading = $null
if (Test-Path -LiteralPath $ledgerPath -PathType Leaf) {
    try { $priorReading = Get-Content -LiteralPath $ledgerPath -Raw -Encoding UTF8 | ConvertFrom-Json } catch { $priorReading = $null }
}

if ($priorReading) {
    $deltaPins = Format-Delta $priorReading.pin_windows $pinWindowCount
    $deltaWorkingSet = Format-Delta $priorReading.working_set_mib $workingSetMib
    $deltaPrivate = Format-Delta $priorReading.private_mib $privateMib
    $deltaGdi = Format-Delta $priorReading.gdi_objects $gdiText
    Write-Output "[pin-memory] vs previous (label=$($priorReading.label) pin_windows=$($priorReading.pin_windows)): d_pin_windows=$deltaPins d_working_set_mib=$deltaWorkingSet d_private_mib=$deltaPrivate d_gdi_objects=$deltaGdi"
} else {
    Write-Output "[pin-memory] no previous reading in $ledgerPath; this one starts the ledger"
}

$reading = [ordered]@{
    label           = $labelText
    pid             = $processId
    pin_windows     = $pinWindowCount
    working_set_mib = $workingSetMib
    private_mib     = $privateMib
    gdi_objects     = $gdiText
    at              = (Get-Date).ToString('s')
}
try {
    $reading | ConvertTo-Json -Compress | Set-Content -LiteralPath $ledgerPath -Encoding UTF8
} catch {
    Write-Output "[pin-memory] warning: could not write the ledger at $ledgerPath ($_)"
}

if ($Details) {
    $threadCount = $process.Threads.Count
    $handleCount = $process.HandleCount
    Write-Output "[pin-memory] threads=$threadCount handles=$handleCount"
}
