using System;
using System.Collections.ObjectModel;
using System.IO;
using System.Net.Http;
using System.Net.Http.Headers;
using System.Net.WebSockets;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;
using System.Threading;
using System.Threading.Tasks;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;

namespace MeowLiveChatWidget;

public partial class MainWindow : Window
{
    private const int DWMWA_WINDOW_CORNER_PREFERENCE = 33;
    private const int DWMWCP_ROUND = 2;
    private const int DWMWA_SYSTEMBACKDROP_TYPE = 38;
    private const int DWMSBT_MICA = 2;
    private const int DWMSBT_TABBEDWINDOW = 4;
    private readonly HttpClient _http = new();
    private ClientWebSocket? _ws;
    private CancellationTokenSource? _wsCts;
    private bool _lockBottom = true;
    private readonly WidgetConfig _config;

    public ObservableCollection<ChatItem> Messages { get; } = new();

    public MainWindow()
    {
        InitializeComponent();
        DataContext = this;
        _config = WidgetConfig.Load();
        TopmostToggle.IsChecked = false;
        Loaded += OnLoaded;
        Closed += OnClosed;
    }

    protected override void OnSourceInitialized(EventArgs e)
    {
        base.OnSourceInitialized(e);
        EnableBlur(this);
        ApplyRoundedCorners();
    }

    private void ApplyRoundedCorners()
    {
        var hwnd = new System.Windows.Interop.WindowInteropHelper(this).Handle;
        if (hwnd == IntPtr.Zero) return;
        var pref = DWMWCP_ROUND;
        _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, ref pref, sizeof(int));
        var backdrop = DWMSBT_TABBEDWINDOW;
        _ = DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, ref backdrop, sizeof(int));
    }

    [DllImport("dwmapi.dll")]
    private static extern int DwmSetWindowAttribute(IntPtr hwnd, int attr, ref int attrValue, int attrSize);

    private async void OnLoaded(object sender, RoutedEventArgs e)
    {
        await FetchLatestAsync();
        _ = ConnectWebSocketLoopAsync();
    }

    private void OnClosed(object? sender, EventArgs e)
    {
        _wsCts?.Cancel();
        _ws?.Dispose();
        _http.Dispose();
    }

    private void OnHeaderMouseDown(object sender, MouseButtonEventArgs e)
    {
        if (e.ChangedButton == MouseButton.Left)
        {
            DragMove();
        }
    }

    private void OnTopmostToggle(object sender, RoutedEventArgs e)
    {
        Topmost = TopmostToggle.IsChecked == true;
        TopmostToggle.Content = Topmost ? "置顶" : "置顶";
    }

    private void OnCloseClick(object sender, RoutedEventArgs e)
    {
        Close();
    }

    private void OnScrollBottom(object sender, RoutedEventArgs e)
    {
        ScrollToBottom();
        ScrollBottomButton.Visibility = Visibility.Collapsed;
    }

    private void OnChatScroll(object sender, ScrollChangedEventArgs e)
    {
        if (ChatScroll.VerticalOffset >= ChatScroll.ScrollableHeight - 16)
        {
            _lockBottom = true;
            ScrollBottomButton.Visibility = Visibility.Collapsed;
        }
        else
        {
            _lockBottom = false;
        }
    }

    private async Task FetchLatestAsync()
    {
        try
        {
            var url = new Uri(new Uri(_config.ApiBase), "/api/chat/latest");
            using var res = await _http.GetAsync(url);
            if (!res.IsSuccessStatusCode) return;
            var json = await res.Content.ReadAsStringAsync();
            var doc = JsonDocument.Parse(json);
            if (!doc.RootElement.TryGetProperty("items", out var items)) return;
            Application.Current.Dispatcher.Invoke(() =>
            {
                Messages.Clear();
                foreach (var item in items.EnumerateArray())
                {
                    var user = item.GetProperty("user").GetString() ?? "";
                    var text = item.GetProperty("text").GetString() ?? "";
                    Messages.Add(new ChatItem(user, text));
                }
                ScrollToBottom();
            });
        }
        catch
        {
            // ignore
        }
    }

    private async Task ConnectWebSocketLoopAsync()
    {
        while (true)
        {
            try
            {
                StatusText.Text = "连接中";
                _wsCts = new CancellationTokenSource();
                _ws = new ClientWebSocket();
                await _ws.ConnectAsync(new Uri(_config.WsUrl), _wsCts.Token);
                StatusText.Text = "已连接";
                await ReceiveLoopAsync(_ws, _wsCts.Token);
            }
            catch
            {
                StatusText.Text = "重连中";
            }

            await Task.Delay(2000);
        }
    }

    private async Task ReceiveLoopAsync(ClientWebSocket ws, CancellationToken ct)
    {
        var buffer = new byte[64 * 1024];
        while (ws.State == WebSocketState.Open && !ct.IsCancellationRequested)
        {
            var result = await ws.ReceiveAsync(buffer, ct);
            if (result.MessageType == WebSocketMessageType.Close)
            {
                await ws.CloseAsync(WebSocketCloseStatus.NormalClosure, "", ct);
                break;
            }

            var text = Encoding.UTF8.GetString(buffer, 0, result.Count);
            try
            {
                HandleWsMessage(text);
            }
            catch
            {
                // ignore
            }
        }
    }

    private void HandleWsMessage(string text)
    {
        using var doc = JsonDocument.Parse(text);
        var root = doc.RootElement;
        if (!root.TryGetProperty("type", out var typeEl)) return;
        var type = typeEl.GetString() ?? "";
        if (type == "chat:new" && root.TryGetProperty("data", out var data))
        {
            var user = data.GetProperty("user").GetString() ?? "";
            var msg = data.GetProperty("text").GetString() ?? "";
            Application.Current.Dispatcher.Invoke(() =>
            {
                Messages.Add(new ChatItem(user, msg));
                TrimMessages();
                if (_lockBottom)
                {
                    ScrollToBottom();
                }
                else
                {
                    ScrollBottomButton.Visibility = Visibility.Visible;
                }
            });
        }
        else if (type == "snapshot" && root.TryGetProperty("data", out var snapshot))
        {
            if (!snapshot.TryGetProperty("chat", out var chat) || !chat.TryGetProperty("items", out var items))
                return;
            Application.Current.Dispatcher.Invoke(() =>
            {
                Messages.Clear();
                foreach (var item in items.EnumerateArray())
                {
                    var user = item.GetProperty("user").GetString() ?? "";
                    var msg = item.GetProperty("text").GetString() ?? "";
                    Messages.Add(new ChatItem(user, msg));
                }
                ScrollToBottom();
            });
        }
    }

    private void TrimMessages()
    {
        while (Messages.Count > 200)
        {
            Messages.RemoveAt(0);
        }
    }

    private void ScrollToBottom()
    {
        ChatScroll.ScrollToEnd();
    }

    private async void OnSendClick(object sender, RoutedEventArgs e)
    {
        await SendChatAsync();
    }

    private async void OnChatInputKeyDown(object sender, KeyEventArgs e)
    {
        if (e.Key == Key.Enter)
        {
            await SendChatAsync();
            e.Handled = true;
        }
    }

    private async Task SendChatAsync()
    {
        var text = ChatInput.Text?.Trim() ?? "";
        if (string.IsNullOrWhiteSpace(text))
        {
            NoticeText.Text = "请输入弹幕内容";
            return;
        }
        if (string.IsNullOrWhiteSpace(_config.ViewerToken))
        {
            NoticeText.Text = "请在 settings.json 配置 ViewerToken";
            return;
        }

        try
        {
            var url = new Uri(new Uri(_config.ApiBase), "/api/chat/send");
            var payload = JsonSerializer.Serialize(new { text });
            using var req = new HttpRequestMessage(HttpMethod.Post, url);
            req.Headers.Authorization = new AuthenticationHeaderValue("Bearer", _config.ViewerToken);
            req.Content = new StringContent(payload, Encoding.UTF8, "application/json");
            using var res = await _http.SendAsync(req);
            if (!res.IsSuccessStatusCode)
            {
                NoticeText.Text = $"发送失败 ({(int)res.StatusCode})";
                return;
            }
            ChatInput.Text = "";
            NoticeText.Text = "已发送";
        }
        catch
        {
            NoticeText.Text = "发送失败";
        }
    }

    private static void EnableBlur(Window window)
    {
        var hwnd = new System.Windows.Interop.WindowInteropHelper(window).Handle;
        var accent = new AccentPolicy
        {
            AccentState = AccentState.ACCENT_ENABLE_BLURBEHIND,
            GradientColor = unchecked((int)0xCC121218)
        };
        var accentSize = Marshal.SizeOf(accent);
        var accentPtr = Marshal.AllocHGlobal(accentSize);
        Marshal.StructureToPtr(accent, accentPtr, false);
        var data = new WindowCompositionAttributeData
        {
            Attribute = WindowCompositionAttribute.WCA_ACCENT_POLICY,
            SizeOfData = accentSize,
            Data = accentPtr
        };
        SetWindowCompositionAttribute(hwnd, ref data);
        Marshal.FreeHGlobal(accentPtr);
    }

    [DllImport("user32.dll")]
    private static extern int SetWindowCompositionAttribute(IntPtr hwnd, ref WindowCompositionAttributeData data);

    private enum AccentState
    {
        ACCENT_DISABLED = 0,
        ACCENT_ENABLE_BLURBEHIND = 3,
        ACCENT_ENABLE_ACRYLICBLURBEHIND = 4
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct AccentPolicy
    {
        public AccentState AccentState;
        public int AccentFlags;
        public int GradientColor;
        public int AnimationId;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct WindowCompositionAttributeData
    {
        public WindowCompositionAttribute Attribute;
        public IntPtr Data;
        public int SizeOfData;
    }

    private enum WindowCompositionAttribute
    {
        WCA_ACCENT_POLICY = 19
    }
}

public record ChatItem(string User, string Text);

public record WidgetConfig(string ApiBase, string WsUrl, string ViewerToken)
{
    public static WidgetConfig Load()
    {
        var envApi = Environment.GetEnvironmentVariable("MEOW_WIDGET_API_BASE");
        var envWs = Environment.GetEnvironmentVariable("MEOW_WIDGET_WS_URL");
        var envToken = Environment.GetEnvironmentVariable("MEOW_WIDGET_VIEWER_TOKEN");
        var filePath = Path.Combine(AppContext.BaseDirectory, "settings.json");
        if (File.Exists(filePath))
        {
            try
            {
                var json = File.ReadAllText(filePath);
                var doc = JsonDocument.Parse(json);
                var apiBase = doc.RootElement.TryGetProperty("ApiBase", out var apiEl)
                    ? apiEl.GetString()
                    : null;
                var wsUrl = doc.RootElement.TryGetProperty("WsUrl", out var wsEl)
                    ? wsEl.GetString()
                    : null;
                var token = doc.RootElement.TryGetProperty("ViewerToken", out var tokenEl)
                    ? tokenEl.GetString()
                    : null;
                return Normalize(envApi ?? apiBase ?? "http://127.0.0.1:5174", envWs ?? wsUrl, envToken ?? token);
            }
            catch
            {
                // ignore
            }
        }
        return Normalize(envApi ?? "http://127.0.0.1:5174", envWs, envToken);
    }

    private static WidgetConfig Normalize(string apiBase, string? wsUrl, string? viewerToken)
    {
        if (string.IsNullOrWhiteSpace(apiBase))
        {
            apiBase = "http://127.0.0.1:5174";
        }
        apiBase = apiBase.Trim().TrimEnd('/');
        if (string.IsNullOrWhiteSpace(wsUrl))
        {
            var ws = apiBase.StartsWith("https", StringComparison.OrdinalIgnoreCase) ? "wss" : "ws";
            wsUrl = $"{ws}://{apiBase.Replace("http://", "").Replace("https://", "")}/ws";
        }
        return new WidgetConfig(apiBase, wsUrl, viewerToken ?? "");
    }
}
