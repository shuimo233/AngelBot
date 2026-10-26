// Owned WPF fixture for scripts/native_uia_smoke.py. Never drives another app.
using System;
using System.Diagnostics;
using System.IO;
using System.Web.Script.Serialization;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Media;
using System.Windows.Interop;
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
    private static CheckBox checkbox;
    private static MenuItem moreActions;
    private static ScrollViewer scrollArea;
    private static int invocationCount;
    private static int selectionCount;
    private static int checkedCount;
    private static int uncheckedCount;
    private static int indeterminateCount;
    private static int expansionCount;
    private static int collapseCount;
    private static int scrollCount;
    private static int mouseMoveCount;
    private static int mouseDownCount;
    private static int mouseWheelCount;
    private static int keyDownCount;
    private static int visualClickCount;
    private static bool visualClickAtCenter;
    private static string snapshotPath;
    private static string windowTitle;
    private static Window ownedWindow;
    private static HwndSource auxiliaryWindow;
    private static Window secondWindow;
    private static Grid densePanel;
    private static GroupBox[] denseRegions;
    private static TextBox[] denseFields;
    private static CheckBox[] denseCheckboxes;
    private static int[] denseFieldChangeCounts;
    private static int[] denseCheckedCounts;
    private static int[] denseUncheckedCounts;
    private static int[] denseButtonInvocationCounts;
    private static bool denseMode;
    private static bool denseSwapped;
    private static object legacyScrollCheckpoint;
    private static ComboBox choicesCombo;
    private static ListBox choicesList;
    private static int choicesSelectionCount;
    private static int choicesExpansionCount;
    private static int choicesCollapseCount;
    private static int choicesComboSelectionCount;

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
            windowState = ownedWindow == null ? "Unavailable" : ownedWindow.WindowState.ToString(),
            auxiliaryOpen = auxiliaryWindow != null,
            secondWindowOpen = secondWindow != null && secondWindow.IsVisible,
            mouseMoveCount = mouseMoveCount,
            mouseDownCount = mouseDownCount,
            mouseWheelCount = mouseWheelCount,
            keyDownCount = keyDownCount,
            visualClickCount = visualClickCount,
            visualClickAtCenter = visualClickAtCenter,
            ordinaryValue = ordinary.Text,
            ordinaryReadOnly = ordinary.IsReadOnly,
            readOnlyValue = readOnly.Text,
            duplicateOneValue = duplicateOne.Text,
            duplicateTwoValue = duplicateTwo.Text,
            passwordUnchanged = password.Password == PasswordSeed,
            invocationCount = invocationCount,
            selected = choice.IsChecked == true,
            checkboxChecked = checkbox.IsChecked,
            checkboxState = checkbox.IsChecked == null ? "indeterminate"
                : checkbox.IsChecked == true ? "on" : "off",
            checkedCount = checkedCount,
            uncheckedCount = uncheckedCount,
            indeterminateCount = indeterminateCount,
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
            denseMode = denseMode,
            denseSwapped = denseSwapped,
            legacyScrollCheckpoint = legacyScrollCheckpoint,
            choicesReady = choicesCombo != null,
            choicesExpanded = choicesCombo != null && choicesCombo.IsDropDownOpen,
            choicesSelected = choicesList != null && choicesList.SelectedIndex == 1,
            choicesSelectionCount = choicesSelectionCount,
            choicesExpansionCount = choicesExpansionCount,
            choicesCollapseCount = choicesCollapseCount,
            choicesComboSelected = choicesCombo != null && choicesCombo.SelectedIndex == 1,
            choicesComboSelectionCount = choicesComboSelectionCount,
            denseFieldValues = denseFields == null ? null : Array.ConvertAll(denseFields, field => field.Text),
            denseChecked = denseCheckboxes == null ? null : Array.ConvertAll(denseCheckboxes, field => field.IsChecked == true),
            denseFieldChangeCounts = denseFieldChangeCounts,
            denseCheckedCounts = denseCheckedCounts,
            denseUncheckedCounts = denseUncheckedCounts,
            denseButtonInvocationCounts = denseButtonInvocationCounts,
        };
        // Same UI thread owns every write. Readers retry transient partial JSON.
        File.WriteAllText(snapshotPath, new JavaScriptSerializer().Serialize(data));
    }

    private static void RememberLegacyScroll()
    {
        if (legacyScrollCheckpoint != null) return;
        legacyScrollCheckpoint = new {
            scrollOffset = scrollArea.VerticalOffset,
            scrollableHeight = scrollArea.ScrollableHeight,
            scrollViewportHeight = scrollArea.ViewportHeight,
            scrollHorizontalOffset = scrollArea.HorizontalOffset,
            scrollCount = scrollCount,
            scrollAtTop = scrollArea.VerticalOffset == 0,
        };
    }

    private static void ShowChoices()
    {
        if (choicesCombo != null) return;
        RememberLegacyScroll();
        var panel = new StackPanel { Margin = new Thickness(12) };
        choicesCombo = new ComboBox { Width = 240, Height = 32, HorizontalAlignment = HorizontalAlignment.Left };
        AutomationProperties.SetAutomationId(choicesCombo, "fixtureCombo");
        AutomationProperties.SetName(choicesCombo, "Fixture choices");
        choicesCombo.Items.Add("Combo option one");
        choicesCombo.Items.Add("Combo option two");
        choicesCombo.SelectionChanged += delegate { choicesComboSelectionCount++; Snapshot(); };
        choicesCombo.DropDownOpened += delegate { choicesExpansionCount++; Snapshot(); };
        choicesCombo.DropDownClosed += delegate { choicesCollapseCount++; Snapshot(); };
        choicesList = new ListBox { Width = 240, Height = 90, Margin = new Thickness(0, 8, 0, 0), HorizontalAlignment = HorizontalAlignment.Left };
        AutomationProperties.SetName(choicesList, "Fixture choice list");
        // A same-name item in a different container must never be chosen.
        foreach (string name in new[] { "Fixture first option", "Fixture second option", "Combo option two" }) {
            var item = new ListBoxItem { Content = name };
            AutomationProperties.SetName(item, name);
            choicesList.Items.Add(item);
        }
        choicesList.SelectionChanged += delegate { choicesSelectionCount++; Snapshot(); };
        panel.Children.Add(choicesCombo);
        panel.Children.Add(choicesList);
        ownedWindow.Content = panel;
        Snapshot();
    }

    private static void ShowDenseRegions()
    {
        if (denseMode) return;
        // Removing the old panel can reset ScrollViewer layout metrics. Keep
        // this explicitly named checkpoint, not a fabricated live readback.
        RememberLegacyScroll();
        densePanel = new Grid { Margin = new Thickness(8) };
        densePanel.RowDefinitions.Add(new RowDefinition());
        densePanel.RowDefinitions.Add(new RowDefinition());
        densePanel.ColumnDefinitions.Add(new ColumnDefinition());
        densePanel.ColumnDefinitions.Add(new ColumnDefinition());
        denseRegions = new GroupBox[4];
        denseFields = new TextBox[4];
        denseCheckboxes = new CheckBox[4];
        denseFieldChangeCounts = new int[4];
        denseCheckedCounts = new int[4];
        denseUncheckedCounts = new int[4];
        denseButtonInvocationCounts = new int[4];
        for (int index = 0; index < 4; index++) {
            int regionIndex = index;
            string suffix = ((char)('A' + index)).ToString();
            var content = new Grid { Margin = new Thickness(3) };
            for (int row = 0; row < 6; row++) content.RowDefinitions.Add(new RowDefinition());
            content.ColumnDefinitions.Add(new ColumnDefinition());
            content.ColumnDefinitions.Add(new ColumnDefinition());
            denseFields[index] = Field("regionField", "Region field", "region-" + suffix + "-seed", false);
            denseFields[index].Height = 26;
            denseFields[index].Margin = new Thickness(2);
            denseFields[index].TextChanged += delegate { denseFieldChangeCounts[regionIndex]++; Snapshot(); };
            content.Children.Add(denseFields[index]);
            denseCheckboxes[index] = new CheckBox { Content = "Region checkbox", Height = 26, Margin = new Thickness(2) };
            AutomationProperties.SetAutomationId(denseCheckboxes[index], "regionCheckbox");
            AutomationProperties.SetName(denseCheckboxes[index], "Region checkbox");
            denseCheckboxes[index].Checked += delegate { denseCheckedCounts[regionIndex]++; Snapshot(); };
            denseCheckboxes[index].Unchecked += delegate { denseUncheckedCounts[regionIndex]++; Snapshot(); };
            Grid.SetColumn(denseCheckboxes[index], 1);
            content.Children.Add(denseCheckboxes[index]);
            for (int buttonIndex = 0; buttonIndex < 10; buttonIndex++) {
                var button = new Button { Content = "Action " + (buttonIndex + 1), Height = 26, Margin = new Thickness(2) };
                AutomationProperties.SetAutomationId(button, "regionButton" + buttonIndex);
                AutomationProperties.SetName(button, "Region button " + (buttonIndex + 1));
                button.Click += delegate { denseButtonInvocationCounts[regionIndex]++; Snapshot(); };
                Grid.SetRow(button, (buttonIndex + 2) / 2);
                Grid.SetColumn(button, (buttonIndex + 2) % 2);
                content.Children.Add(button);
            }
            var region = new GroupBox {
                Header = "Fixture region " + suffix, Content = content, Margin = new Thickness(4),
                // Known opaque owned pixels for native WGC acceptance; no new
                // controls, values, observation budget or user content.
                Background = new SolidColorBrush(index % 2 == 0
                    ? Color.FromRgb(17, 201, 221) : Color.FromRgb(239, 31, 177)),
            };
            AutomationProperties.SetAutomationId(region, "fixtureRegion" + suffix);
            AutomationProperties.SetName(region, "Fixture region " + suffix);
            Grid.SetRow(region, index / 2);
            Grid.SetColumn(region, index % 2);
            denseRegions[index] = region;
            densePanel.Children.Add(region);
        }
        // An owned, constant-color canvas: no InvokePattern, focus animation,
        // user content, key logging or recorded pointer coordinates.
        var canary = new Border { Width = 20, Height = 20, Background = new SolidColorBrush(Color.FromRgb(7, 59, 173)),
            HorizontalAlignment = HorizontalAlignment.Left, VerticalAlignment = VerticalAlignment.Bottom, Margin = new Thickness(14) };
        canary.MouseLeftButtonDown += delegate(object sender, System.Windows.Input.MouseButtonEventArgs e) {
            var point = e.GetPosition(canary);
            visualClickCount++;
            visualClickAtCenter = Math.Abs(point.X - canary.ActualWidth / 2) <= 1.5 && Math.Abs(point.Y - canary.ActualHeight / 2) <= 1.5;
            Snapshot();
        };
        Grid.SetRow(canary, 0); Grid.SetColumn(canary, 0); Panel.SetZIndex(canary, 10); densePanel.Children.Add(canary);
        denseMode = true;
        ownedWindow.Content = densePanel;
        Snapshot();
    }

    private static void SwapDenseRegions()
    {
        if (!denseMode || denseSwapped) return;
        // Grid position alone does not change UIA ControlView sibling paths.
        // Reorder the actual child collection without recreating either node.
        densePanel.Children.Remove(denseRegions[1]);
        densePanel.Children.Insert(0, denseRegions[1]);
        Grid.SetColumn(denseRegions[0], 1);
        Grid.SetColumn(denseRegions[1], 0);
        denseSwapped = true;
        Snapshot();
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
        checkbox = new CheckBox { Content = "Fixture checkbox", Height = 25, Margin = new Thickness(4) };
        AutomationProperties.SetAutomationId(checkbox, "fixtureCheckbox");
        AutomationProperties.SetName(checkbox, "Fixture checkbox");
        checkbox.Checked += delegate { checkedCount++; Snapshot(); };
        checkbox.Unchecked += delegate { uncheckedCount++; Snapshot(); };
        checkbox.Indeterminate += delegate { indeterminateCount++; Snapshot(); };
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
        panel.Children.Add(checkbox);
        panel.Children.Add(menu);
        panel.Children.Add(scrollArea);
        var window = new Window {
            Title = windowTitle,
            Width = 440,
            Height = 512,
            Content = panel,
            ShowActivated = false,
            WindowStartupLocation = WindowStartupLocation.CenterScreen,
        };
        ownedWindow = window;
        // Own-window input counts distinguish incidental desktop interaction
        // from UIA pattern effects. Never record keys, positions or other apps.
        // Let existing state snapshots publish these counts; do not change
        // event routing or add disk writes on every pointer movement.
        window.PreviewMouseMove += delegate { mouseMoveCount++; };
        window.PreviewMouseDown += delegate { mouseDownCount++; };
        window.PreviewMouseWheel += delegate { mouseWheelCount++; };
        window.PreviewKeyDown += delegate { keyDownCount++; };
        ordinary.TextChanged += delegate { Snapshot(); };
        readOnly.TextChanged += delegate { Snapshot(); };
        duplicateOne.TextChanged += delegate { Snapshot(); };
        duplicateTwo.TextChanged += delegate { Snapshot(); };
        password.PasswordChanged += delegate { Snapshot(); };
        window.ContentRendered += delegate { Snapshot(); };
        window.StateChanged += delegate { Snapshot(); };
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
            if (command != "read-only" && command != "editable" && command != "checkbox-indeterminate"
                && command != "dense" && command != "dense-swap" && command != "choices"
                && command != "minimize" && command != "restore"
                && command != "auxiliary" && command != "second-window" && command != "extras-close") return;
            previousCommand = command;
            if (command == "auxiliary") {
                if (auxiliaryWindow == null) {
                    // Real unowned, visible, non-activating tool HWND. This
                    // reproduces popup-like discovery without touching another app.
                    var options = new HwndSourceParameters("AngelBot owned auxiliary") {
                        Width = 120, Height = 28,
                        PositionX = (int)window.Left + 8, PositionY = (int)window.Top + 80,
                        WindowStyle = unchecked((int)0x90000000),
                        ExtendedWindowStyle = 0x08000080,
                        ParentWindow = IntPtr.Zero,
                    };
                    auxiliaryWindow = new HwndSource(options);
                    auxiliaryWindow.RootVisual = new Border {
                        Background = Brushes.LightGray,
                        Child = new TextBlock { Text = "Owned auxiliary" },
                    };
                }
            } else if (command == "second-window") {
                if (secondWindow == null) {
                    secondWindow = new Window {
                        Title = windowTitle + " second", Width = 200, Height = 100,
                        ShowActivated = false, Content = new TextBlock { Text = "Independent owned window" },
                    };
                    secondWindow.Show();
                }
            } else if (command == "extras-close") {
                if (secondWindow != null) { secondWindow.Close(); secondWindow = null; }
                if (auxiliaryWindow != null) { auxiliaryWindow.Dispose(); auxiliaryWindow = null; }
            } else if (command == "minimize") {
                window.WindowState = WindowState.Minimized;
            } else if (command == "restore") {
                window.WindowState = WindowState.Normal;
            } else if (command == "dense") {
                ShowDenseRegions();
            } else if (command == "choices") {
                ShowChoices();
            } else if (command == "dense-swap") {
                SwapDenseRegions();
            } else if (command == "checkbox-indeterminate") {
                checkbox.IsThreeState = true;
                checkbox.IsChecked = null;
            } else {
                ordinary.IsReadOnly = command == "read-only";
            }
            Snapshot();
        };
        commands.Start();
        // Defensive expiry if the runner itself is interrupted or crashes.
        // The owning launcher normally closes this child. Allow real-model
        // turns and preview review; this timer only bounds an orphaned fixture.
        var expiry = new DispatcherTimer { Interval = TimeSpan.FromMinutes(15) };
        expiry.Tick += delegate { window.Close(); };
        expiry.Start();
        new Application().Run(window);
    }
}
