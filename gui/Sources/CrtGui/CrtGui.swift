import AppKit
import SwiftUI

private var retainedDelegate: AppDelegate?

@_cdecl("crt_gui_run")
public func crtGuiRun(_ version: UnsafePointer<CChar>?) {
    let version = version.map { String(cString: $0) } ?? ""
    let app = NSApplication.shared
    app.setActivationPolicy(.regular)

    let delegate = AppDelegate(version: version)
    retainedDelegate = delegate
    app.delegate = delegate
    app.mainMenu = makeMainMenu()
    app.run()
}

private func makeMainMenu() -> NSMenu {
    let mainMenu = NSMenu()
    let appItem = NSMenuItem()
    mainMenu.addItem(appItem)

    let appMenu = NSMenu(title: "crt")
    appMenu.addItem(
        withTitle: "Quit crt",
        action: #selector(NSApplication.terminate(_:)),
        keyEquivalent: "q"
    )
    appItem.submenu = appMenu
    return mainMenu
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    private let version: String
    private var window: NSWindow?

    init(version: String) {
        self.version = version
        super.init()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        let hosting = NSHostingView(rootView: ContentView(version: version))
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 640, height: 420),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = "Code Review Tool"
        window.contentView = hosting
        window.center()
        window.makeKeyAndOrderFront(nil)
        self.window = window
        NSApp.activate(ignoringOtherApps: true)
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }
}
