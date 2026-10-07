// Owned WPF fixture for scripts/native_uia_smoke.py. Never drives another app.
using System;
using System.Diagnostics;
using System.IO;
using System.Web.Script.Serialization;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Threading;

internal static class NativeUiaFixture
{
    private const string PasswordSeed = "native-uia-password-sentinel";
    private static TextBox ordinary;
    private static TextBox readOnly;
    private static TextBox duplicateOne;
    private static TextBox duplicateTwo;
    private static PasswordBox password;
    private static Button actionButton;
    private static RadioButton choice;
    private static MenuItem moreActions;
    private static ScrollViewer scrollArea;
    private static int invocationCount;
    private static int selectionCount;
    private static int expansionCount;
    private static int collapseCount;
    private static int scrollCount;
    private static string snapshotPath;
    private static string windowTitle;

    private static TextBox Field(string id, string name, string seed, bool isReadOnly)
    {
        var field = new TextBox {
            Text = seed,
            IsReadOnly = isReadOnly,
            Height = 30,
            Margin = new Thickness(4),
        };
        AutomationProperties.SetAutomationId(field, id);
        AutomationProperties.SetName(field, name);
        return field;
    }

    private static void Snapshot()
    {
        if (ordinary == null || password == null || scrollArea == null) return;
        var data = new {
            processId = Process.GetCurrentProcess().Id,
            windowTitle = windowTitle,
            ordinaryValue = ordinary.Text,
            ordinaryReadOnly = ordinary.IsReadOnly,
            readOnlyValue = readOnly.Text,
            duplicateOneValue = duplicateOne.Text,
            duplicateTwoValue = duplicateTwo.Text,
            passwordUnchanged = password.Password == PasswordSeed,
            invocationCount = invocationCount,
            selected = choice.IsChecked == true,
            menuExpanded = moreActions.IsSubmenuOpen,
            selectionCount = selectionCount,
            expansionCount = expansionCount,
            collapseCount = collapseCount,
            scrollOffset = scrollArea.VerticalOffset,
            scrollableHeight = scrollArea.ScrollableHeight,
            scrollViewportHeight = scrollArea.ViewportHeight,
            scrollHorizontalOffset = scrollArea.HorizontalOffset,
            scrollCount = scrollCount,
            scrollAtTop = scrollArea.VerticalOffset == 0,
            scrollAtBottom = scrollArea.ScrollableHeight > 0
                && Math.Abs(scrollArea.VerticalOffset - scrollArea.ScrollableHeight) < 0.001,
        };
        // Same UI thread owns every write. Readers retry transient partial JSON.
        File.WriteAllText(snapshotPath, new JavaScriptSerializer().Serialize(data));
    }

    [STAThread]
    public static void Main(string[] args)
    {
        if (args.Length != 2) throw new ArgumentException("Expected snapshot path and unique title");
        snapshotPath = args[0];
        windowTitle = args[1];
        ordinary = Field("ordinaryField", "Ordinary field", "ordinary-seed", false);
        readOnly = Field("readOnlyField", "Read-only field", "read-only-seed", true);
        duplicateOne = Field("duplicateField", "Duplicate field", "duplicate-one-seed", false);
        duplicateTwo = Field("duplicateField", "Duplicate field", "duplicate-two-seed", false);
        password = new PasswordBox {
            Password = PasswordSeed,
            Height = 30,
            Margin = new Thickness(4),
        };
        AutomationProperties.SetAutomationId(password, "passwordField");
        AutomationProperties.SetName(password, "Password field");
        actionButton = new Button { Content = "Apply action", Height = 30, Margin = new Thickness(4) };
        AutomationProperties.SetAutomationId(actionButton, "actionButton");
        AutomationProperties.SetName(actionButton, "Apply action");
        actionButton.Click += delegate {
            invocationCount++;
            actionButton.Content = "Action applied";
            AutomationProperties.SetName(actionButton, "Action applied");
            Snapshot();
        };
        choice = new RadioButton { Content = "Fixture option", Height = 25, Margin = new Thickness(4) };
        AutomationProperties.SetAutomationId(choice, "fixtureOption");
        AutomationProperties.SetName(choice, "Fixture option");
        choice.Checked += delegate { selectionCount++; Snapshot(); };
        moreActions = new MenuItem { Header = "More actions" };
        AutomationProperties.SetAutomationId(moreActions, "fixtureMenu");
        AutomationProperties.SetName(moreActions, "More actions");
        moreActions.Items.Add(new MenuItem { Header = "Fixture menu item" });
        moreActions.SubmenuOpened += delegate { expansionCount++; Snapshot(); };
        moreActions.SubmenuClosed += delegate { collapseCount++; Snapshot(); };
        var menu = new Menu { Height = 26, Margin = new Thickness(4) };
        menu.Items.Add(moreActions);
        // A small overflow reaches either edge in one native scroll increment.
        // Hidden bars keep the owned Control View bounded without hiding content.
        var scrollContent = new StackPanel { Height = 84 };
        scrollContent.Children.Add(new TextBlock { Text = "Fixture scroll start", Height = 42 });
        scrollContent.Children.Add(new TextBlock { Text = "Fixture scroll end", Height = 42 });
        scrollArea = new ScrollViewer {
            Content = scrollContent,
            Height = 80,
            Margin = new Thickness(4),
            CanContentScroll = false,
            VerticalScrollBarVisibility = ScrollBarVisibility.Hidden,
            HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled,
        };
        AutomationProperties.SetAutomationId(scrollArea, "fixtureScroll");
        AutomationProperties.SetName(scrollArea, "Fixture scroll area");
        scrollArea.ScrollChanged += delegate(object sender, ScrollChangedEventArgs e) {
            if (e.VerticalChange != 0) scrollCount++;
            Snapshot();
        };
        var panel = new StackPanel { Margin = new Thickness(8) };
        panel.Children.Add(ordinary);
        panel.Children.Add(readOnly);
        panel.Children.Add(password);
        panel.Children.Add(duplicateOne);
        panel.Children.Add(duplicateTwo);
        panel.Children.Add(actionButton);
        panel.Children.Add(choice);
        panel.Children.Add(menu);
        panel.Children.Add(scrollArea);
        var window = new Window {
            Title = windowTitle,
            Width = 440,
            Height = 480,
            Content = panel,
            ShowActivated = false,
            WindowStartupLocation = WindowStartupLocation.CenterScreen,
        };
        ordinary.TextChanged += delegate { Snapshot(); };
        readOnly.TextChanged += delegate { Snapshot(); };
        duplicateOne.TextChanged += delegate { Snapshot(); };
        duplicateTwo.TextChanged += delegate { Snapshot(); };
        password.PasswordChanged += delegate { Snapshot(); };
        window.ContentRendered += delegate { Snapshot(); };
        // This fixture-only command changes safety state, never another window.
        string commandPath = Path.ChangeExtension(snapshotPath, "command");
        string previousCommand = "";
        var commands = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(50) };
        commands.Tick += delegate {
            if (!File.Exists(commandPath)) return;
            string command;
            try { command = File.ReadAllText(commandPath); }
            catch (IOException) { return; }
            if (command == previousCommand) return;
            if (command != "read-only" && command != "editable") return;
            previousCommand = command;
            ordinary.IsReadOnly = command == "read-only";
            Snapshot();
        };
        commands.Start();
        // Defensive expiry if the runner itself is interrupted or crashes.
        var expiry = new DispatcherTimer { Interval = TimeSpan.FromSeconds(180) };
        expiry.Tick += delegate { window.Close(); };
        expiry.Start();
        new Application().Run(window);
    }
}
