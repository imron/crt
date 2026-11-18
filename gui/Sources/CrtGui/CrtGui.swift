import AppKit
import SwiftUI

private var retainedDelegate: AppDelegate?

@_cdecl("crt_gui_run")
public func crtGuiRun(
    _ version: UnsafePointer<CChar>?,
    _ repoName: UnsafePointer<CChar>?,
    _ worktree: UnsafePointer<CChar>?,
    _ headRef: UnsafePointer<CChar>?,
    _ status: UnsafePointer<CChar>?
) {
    let snapshot = GuiSnapshot(
        version: cString(version),
        repoName: cString(repoName),
        worktree: cString(worktree),
        headRef: cString(headRef),
        status: cString(status)
    )
    let app = NSApplication.shared
    app.setActivationPolicy(.regular)

    let delegate = AppDelegate(snapshot: snapshot)
    retainedDelegate = delegate
    app.delegate = delegate
    app.mainMenu = makeMainMenu()
    app.run()
}

private func cString(_ pointer: UnsafePointer<CChar>?) -> String {
    pointer.map { String(cString: $0) } ?? ""
}

struct GuiSnapshot {
    let version: String
    let repoName: String
    let worktree: String
    let headRef: String
    let status: String
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
    private let snapshot: GuiSnapshot
    private var window: NSWindow?

    init(snapshot: GuiSnapshot) {
        self.snapshot = snapshot
        super.init()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        let content = ContentView(
            version: snapshot.version,
            repoName: snapshot.repoName,
            worktree: snapshot.worktree,
            headRef: snapshot.headRef,
            status: snapshot.status
        )
        let hosting = NSHostingView(rootView: content)
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
