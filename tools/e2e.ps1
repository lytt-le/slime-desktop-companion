# End-to-end interaction test: drives the real mouse against a running slime and records
# what happens into a contact sheet.
#
#   powershell -ExecutionPolicy Bypass -File tools\e2e.ps1 -Class SlimeCompanion -Scenario throw -Out sheet.png
#
# Scenarios: idle, drag, shake, throw, throw_up, drop_window, jump
param(
    [string]$Class = "SlimeCompanion",
    [string]$Scenario = "throw",
    [string]$Out = "e2e.png",
    [int]$Frames = 15,
    [int]$FrameMs = 120,
    [double]$Scale = 0.5,
    # Optional screen region to record, "x,y,w,h" (default: whole screen).
    [string]$Crop = "",
    # Follow the slime: each frame is a box of this size centred on it, at full size.
    [string]$Follow = "",
    # "ride": drag a window by the title-bar point "x,y" down by -RideBy px and back.
    [string]$Title = "",
    [int]$RideBy = 200,
    # "menu": which item to click (-1 = none) and the menu row height.
    [int]$Item = -1,
    [int]$ItemH = 32
)

Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Threading;

public static class E2E {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    [StructLayout(LayoutKind.Sequential)] struct MOUSEINPUT { public int dx, dy; public uint data, flags, time; public IntPtr extra; }
    [StructLayout(LayoutKind.Sequential)] struct INPUT { public uint type; public MOUSEINPUT mi; }
    delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc f, IntPtr l);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassNameW(IntPtr h, System.Text.StringBuilder s, int n);
    [DllImport("user32.dll")] static extern int GetWindowLongW(IntPtr h, int i);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern uint SendInput(uint n, INPUT[] i, int size);
    [DllImport("user32.dll")] static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] static extern bool SetProcessDpiAwarenessContext(IntPtr v);

    public static void Init() { SetProcessDpiAwarenessContext(new IntPtr(-4)); }

    // The slime's main window: matching class, not click-through.
    public static RECT Find(string cls) {
        RECT found = new RECT();
        EnumWindows((h, l) => {
            var sb = new System.Text.StringBuilder(64);
            GetClassNameW(h, sb, 64);
            if (sb.ToString() == cls && IsWindowVisible(h) && (GetWindowLongW(h, -20) & 0x20) == 0) {
                GetWindowRect(h, out found);
                return false;
            }
            return true;
        }, IntPtr.Zero);
        return found;
    }

    static void Btn(uint flag) {
        var i = new INPUT[1];
        i[0].type = 0;
        i[0].mi.flags = flag;
        uint sent = SendInput(1, i, Marshal.SizeOf(typeof(INPUT)));
        if (sent != 1) throw new Exception("SendInput failed, size " + Marshal.SizeOf(typeof(INPUT)));
    }
    public static void Down() { Btn(0x0002); }
    public static void Up() { Btn(0x0004); }
    public static void RClick() { Btn(0x0008); Thread.Sleep(40); Btn(0x0010); }
    public static void Click() { Down(); Thread.Sleep(40); Up(); }
    public static void Move(int x, int y) { SetCursorPos(x, y); }

    // Moves the cursor from a to b over ms milliseconds.
    public static void Glide(int x0, int y0, int x1, int y1, int ms) {
        int steps = Math.Max(1, ms / 8);
        for (int s = 1; s <= steps; s++) {
            Move(x0 + (x1 - x0) * s / steps, y0 + (y1 - y0) * s / steps);
            Thread.Sleep(8);
        }
    }
}
'@
[E2E]::Init()

$cx = 0; $cy = 0; $bw = 1920; $bh = 1080
if ($Crop) { $p = $Crop.Split(","); $cx = [int]$p[0]; $cy = [int]$p[1]; $bw = [int]$p[2]; $bh = [int]$p[3] }
if ($Follow) { $p = $Follow.Split(","); $bw = [int]$p[0]; $bh = [int]$p[1] }
function Grab-Frame {
    $x = $cx; $y = $cy
    if ($Follow) {
        $r = [E2E]::Find($Class)
        $x = [int](($r.L + $r.R) / 2 - $bw / 2); $y = [int](($r.T + $r.B) / 2 - $bh / 2)
    }
    $bmp = New-Object System.Drawing.Bitmap $bw, $bh
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($x, $y, 0, 0, $bmp.Size)
    $g.Dispose()
    $bmp
}

$shots = New-Object System.Collections.Generic.List[System.Drawing.Bitmap]
function Record([int]$n, [int]$ms) {
    for ($k = 0; $k -lt $n; $k++) {
        $shots.Add((Grab-Frame))
        Start-Sleep -Milliseconds $ms
    }
}

function Slime-Center {
    $r = [E2E]::Find($Class)
    if ($r.R -le $r.L) { throw "slime window '$Class' not found" }
    # Aim a little below the middle: always solid goo there.
    @([int](($r.L + $r.R) / 2), [int]($r.T + ($r.B - $r.T) * 0.6))
}

Start-Sleep -Milliseconds 1500  # let it settle before aiming
$c = Slime-Center
$rr = [E2E]::Find($Class)
"start at $($c[0]),$($c[1]) (window $($rr.L),$($rr.T)-$($rr.R),$($rr.B))"
switch ($Scenario) {
    "idle" { Record $Frames $FrameMs }
    "drag" {
        [E2E]::Move($c[0], $c[1]); Start-Sleep -Milliseconds 100; [E2E]::Down()
        $shots.Add((Grab-Frame))
        [E2E]::Glide($c[0], $c[1], 900, 350, 600); $shots.Add((Grab-Frame))
        [E2E]::Glide(900, 350, 1300, 450, 500); $shots.Add((Grab-Frame))
        Start-Sleep -Milliseconds 400; $shots.Add((Grab-Frame))
        [E2E]::Up()
        Record ($Frames - 4) $FrameMs
    }
    "shake" {
        [E2E]::Move($c[0], $c[1]); Start-Sleep -Milliseconds 100; [E2E]::Down()
        [E2E]::Glide($c[0], $c[1], 960, 400, 300)
        for ($k = 0; $k -lt 8; $k++) {
            [E2E]::Glide(960, 400, 1160, 420, 70); $shots.Add((Grab-Frame))
            [E2E]::Glide(1160, 420, 960, 400, 70)
        }
        [E2E]::Up()
        Record ($Frames - 8) $FrameMs
    }
    "throw" {
        [E2E]::Move($c[0], $c[1]); Start-Sleep -Milliseconds 100; [E2E]::Down()
        [E2E]::Glide($c[0], $c[1], 700, 400, 400)
        [E2E]::Glide(700, 400, 1100, 330, 80)
        [E2E]::Up()
        Record $Frames $FrameMs
    }
    "throw_up" {
        [E2E]::Move($c[0], $c[1]); Start-Sleep -Milliseconds 100; [E2E]::Down()
        [E2E]::Glide($c[0], $c[1], 800, 700, 400)
        [E2E]::Glide(800, 700, 950, 300, 70)
        [E2E]::Up()
        Record $Frames $FrameMs
    }
    "drop_window" {
        # Carry it high above the middle of the screen and let go: it should land on a window.
        [E2E]::Move($c[0], $c[1]); Start-Sleep -Milliseconds 100; [E2E]::Down()
        [E2E]::Glide($c[0], $c[1], 1000, 60, 700)
        Start-Sleep -Milliseconds 300
        [E2E]::Up()
        Record $Frames $FrameMs
    }
    "ride" {
        $t = $Title.Split(","); $tx = [int]$t[0]; $ty = [int]$t[1]
        [E2E]::Move($tx, $ty); Start-Sleep -Milliseconds 100; [E2E]::Down()
        for ($k = 1; $k -le 6; $k++) { [E2E]::Glide($tx, $ty + ($k - 1) * $RideBy / 6, $tx, $ty + $k * $RideBy / 6, 120); $shots.Add((Grab-Frame)) }
        Start-Sleep -Milliseconds 300; $shots.Add((Grab-Frame))
        for ($k = 5; $k -ge 0; $k--) { [E2E]::Glide($tx, $ty + ($k + 1) * $RideBy / 6, $tx, $ty + $k * $RideBy / 6, 120); $shots.Add((Grab-Frame)) }
        [E2E]::Up()
        Record 2 $FrameMs
    }
    "menu" {
        # Right-click the slime, then pick the menu item -Item (0 = Bigger, 1 = Smaller, 2 = Normal) by offset.
        [E2E]::Move($c[0], $c[1]); Start-Sleep -Milliseconds 100; [E2E]::RClick(); Start-Sleep -Milliseconds 500
        $shots.Add((Grab-Frame))
        if ($Item -ge 0) { [E2E]::Move($c[0] + 40, $c[1] + 18 + $Item * $ItemH); Start-Sleep -Milliseconds 150; [E2E]::Click() }
        Record $Frames $FrameMs
    }
    "jump" {
        # Clicks on empty desktop to the right; it should hop toward each one.
        foreach ($p in @(@(1780, 600), @(1780, 300))) {
            [E2E]::Move($p[0], $p[1]); Start-Sleep -Milliseconds 50
            [E2E]::Down(); Start-Sleep -Milliseconds 30; [E2E]::Up()
            Record ([int]($Frames / 2)) $FrameMs
        }
    }
}

# Contact sheet: frames scaled down, 3 per row.
$fw = [int]($bw * $Scale); $fh = [int]($bh * $Scale)
$cols = 3; $rows = [math]::Ceiling($shots.Count / $cols)
$sheet = New-Object System.Drawing.Bitmap ($fw * $cols), ($fh * $rows)
$g = [System.Drawing.Graphics]::FromImage($sheet)
$g.InterpolationMode = 'HighQualityBilinear'
for ($k = 0; $k -lt $shots.Count; $k++) {
    $g.DrawImage($shots[$k], ($k % $cols) * $fw, [math]::Floor($k / $cols) * $fh, $fw, $fh)
    $g.DrawString("$k", (New-Object System.Drawing.Font "Arial", 14), [System.Drawing.Brushes]::Yellow, ($k % $cols) * $fw + 4, [math]::Floor($k / $cols) * $fh + 4)
    $shots[$k].Dispose()
}
$sheet.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
"saved $Out ($($shots.Count) frames)"
