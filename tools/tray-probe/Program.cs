// tray-probe: asks Windows to show a notification-area icon, several ways, and writes down
// what the shell answered.
//
// Built because the launcher's icon never appears on this machine and `Shell_NotifyIconW`
// keeps returning FALSE with ERROR_ACCESS_DENIED (5), while System.Windows.Forms.NotifyIcon
// succeeds in the same session. Two calls, one process, same environment — so the difference
// is somewhere in the parameters, and this bisects them.
//
// Run it by double-clicking. It writes tray-probe.log next to itself and keeps the last
// successful icon on screen for thirty seconds.

using System;
using System.Drawing;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Windows.Forms;

static class TrayProbe
{
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct NOTIFYICONDATA
    {
        public int cbSize;
        public IntPtr hWnd;
        public uint uID;
        public uint uFlags;
        public uint uCallbackMessage;
        public IntPtr hIcon;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 128)] public string szTip;
        public uint dwState;
        public uint dwStateMask;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 256)] public string szInfo;
        public uint uVersion;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 64)] public string szInfoTitle;
        public uint dwInfoFlags;
        public Guid guidItem;
        public IntPtr hBalloonIcon;
    }

    [DllImport("shell32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool Shell_NotifyIcon(uint message, ref NOTIFYICONDATA data);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    static extern IntPtr LoadIcon(IntPtr instance, IntPtr name);

    const uint NIM_ADD = 0, NIM_MODIFY = 1, NIM_DELETE = 2, NIM_SETVERSION = 4;
    const uint NIF_MESSAGE = 1, NIF_ICON = 2, NIF_TIP = 4, NIF_SHOWTIP = 0x80;

    static readonly StringBuilder Log = new StringBuilder();
    static string LogPath;

    static void Say(string line)
    {
        try { Console.WriteLine(line); } catch { }
        Log.AppendLine(line);
        // Written as we go: a window-less program that crashes should still leave its
        // evidence behind.
        try { File.WriteAllText(LogPath, Log.ToString()); } catch { }
    }

    static string Try(string name, IntPtr hwnd, uint uid, uint flags, uint callback, int cbSize)
    {
        var data = new NOTIFYICONDATA();
        data.cbSize = cbSize;
        data.hWnd = hwnd;
        data.uID = uid;
        data.uFlags = flags;
        data.uCallbackMessage = callback;
        data.hIcon = LoadIcon(IntPtr.Zero, new IntPtr(32512)); // IDI_APPLICATION
        data.szTip = "o824-probe";
        data.szInfo = "";
        data.szInfoTitle = "";
        bool ok = Shell_NotifyIcon(NIM_ADD, ref data);
        int error = Marshal.GetLastWin32Error();
        // A second question to the shell: if the icon really is registered, asking to modify
        // it succeeds. This is the only way to know without looking at the screen, and it is
        // what makes the log self-judging.
        var modify = new NOTIFYICONDATA();
        modify.cbSize = cbSize;
        modify.hWnd = hwnd;
        modify.uID = uid;
        modify.uFlags = NIF_TIP;
        modify.szTip = "o824-probe";
        modify.szInfo = "";
        modify.szInfoTitle = "";
        bool modified = Shell_NotifyIcon(NIM_MODIFY, ref modify);
        int modifyError = Marshal.GetLastWin32Error();

        var result = string.Format(
            "{0,-34} added={1,-5} error={2,-3} proves-registered={3} (modify error={4})",
            name, ok, error, modified, modifyError);
        Say(result);
        if (ok)
        {
            var version = data;
            version.uVersion = 4;
            Shell_NotifyIcon(NIM_SETVERSION, ref version);
        }
        else
        {
            var clear = new NOTIFYICONDATA();
            clear.cbSize = cbSize;
            clear.hWnd = hwnd;
            clear.uID = uid;
            Shell_NotifyIcon(NIM_DELETE, ref clear);
        }
        return result;
    }

    [STAThread]
    static void Main()
    {
        LogPath = Path.Combine(
            Path.GetDirectoryName(Application.ExecutablePath) ?? ".",
            "tray-probe.log");

        Say("== tray probe ==");
        Say("time      : " + DateTime.Now.ToString("yyyy-MM-dd HH:mm:ss"));
        Say("user      : " + Environment.UserName);
        Say("session   : " + System.Diagnostics.Process.GetCurrentProcess().SessionId);
        Say("exe       : " + Application.ExecutablePath);
        Say("notify data size (this build): " + Marshal.SizeOf(typeof(NOTIFYICONDATA)));
        // The integrity level is deliberately not queried here: the marshalling for it crashed
        // a windowless process earlier, and `icacls` already shows the file's mandatory label.
        Say("");

        using (var form = new Form())
        {
            form.ShowInTaskbar = false;
            form.WindowState = FormWindowState.Minimized;
            form.Show();
            var hwnd = form.Handle;

            Say("-- raw Shell_NotifyIcon, message callback 0x401 --");
            Try("flags 0x7  cb 976", hwnd, 1, NIF_MESSAGE | NIF_ICON | NIF_TIP, 0x401, 976);
            Try("flags 0x87 cb 976", hwnd, 2, NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP, 0x401, 976);
            Try("flags 0x7  cb 952", hwnd, 3, NIF_MESSAGE | NIF_ICON | NIF_TIP, 0x401, 952);
            Try("flags 0x7  cb 976 cb0x800", hwnd, 4, NIF_MESSAGE | NIF_ICON | NIF_TIP, 0x800, 976);
            Say("");

            // Hypothesis under test: the shell talks to the target window while adding the
            // icon, so a thread that is not pumping messages cannot be registered with.
            // Everything above ran before the message loop started.
            Say("");
            Say("-- raw Shell_NotifyIcon, once the message loop is running --");
            var insideLoop = new System.Windows.Forms.Timer();
            insideLoop.Interval = 700;
            insideLoop.Tick += (sender, args) =>
            {
                insideLoop.Stop();
                Try("inside loop: flags 0x7 cb 976", hwnd, 10, NIF_MESSAGE | NIF_ICON | NIF_TIP, 0x401, 976);
                Try("inside loop: flags 0x87 cb 976", hwnd, 11, NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP, 0x401, 976);
            };
            insideLoop.Start();

            Say("");
            Say("-- System.Windows.Forms.NotifyIcon (the one that worked before) --");
            try
            {
                using (var icon = new NotifyIcon())
                {
                    icon.Icon = SystemIcons.Application;
                    icon.Text = "o824-probe managed";
                    icon.Visible = true;
                    Say("NotifyIcon.Visible = true (no exception)");
                    Say("");
                    Say("Look at the notification area now: a generic window icon labelled");
                    Say("\"o824-probe managed\" should be there for 30 seconds.");
                    Say("Waiting 30 seconds.");
                    Console.Out.Flush();

                    var stop = new System.Windows.Forms.Timer();
                    stop.Interval = 30000;
                    stop.Tick += (sender, args) => { stop.Stop(); form.Close(); };
                    stop.Start();
                    Application.Run(form);
                    stop.Stop();
                    icon.Visible = false;
                }
            }
            catch (Exception error)
            {
                Say("NotifyIcon threw: " + error.GetType().Name + ": " + error.Message);
            }
        }

        Say("");
        Say("== done ==");
        Say("log written next to the exe: " + LogPath);
    }
}
