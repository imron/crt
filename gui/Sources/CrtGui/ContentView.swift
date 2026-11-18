import SwiftUI

struct ContentView: View {
    @ObservedObject var store: GuiStore
    @State private var selection: String?

    var body: some View {
        HSplitView {
            fileList
                .frame(minWidth: 180, idealWidth: 260, maxWidth: 420)
            diffPane
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(minWidth: 800, minHeight: 480)
        .onAppear {
            selection = store.snapshot.selectedPath
        }
        .onChange(of: store.snapshot.selectedPath) { _, path in
            selection = path
        }
        .onChange(of: selection) { _, path in
            guard let path,
                  path != store.snapshot.selectedPath,
                  store.snapshot.files.contains(where: { $0.path == path })
            else {
                return
            }
            store.selectFile(path)
        }
    }

    private var unreviewed: [GuiFile] {
        store.snapshot.files.filter { $0.status != "reviewed" }
    }

    private var reviewed: [GuiFile] {
        store.snapshot.files.filter { $0.status == "reviewed" }
    }

    private var fileList: some View {
        List(selection: $selection) {
            treeSection(title: "Unreviewed", files: unreviewed)
            treeSection(title: "Reviewed", files: reviewed)
        }
        .listStyle(.sidebar)
    }

    @ViewBuilder
    private func treeSection(title: String, files: [GuiFile]) -> some View {
        if !files.isEmpty {
            Section(title) {
                FileTreeRows(nodes: FileTreeNode.forest(from: files))
            }
        }
    }

    @ViewBuilder
    private var diffPane: some View {
        if let diff = store.snapshot.diff {
            DiffView(diff: diff)
        } else {
            Text("Choose a file from the sidebar.")
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }
}

struct FileTreeRows: View {
    let nodes: [FileTreeNode]

    var body: some View {
        ForEach(nodes) { node in
            FileTreeItem(node: node)
        }
    }
}

struct FileTreeItem: View {
    let node: FileTreeNode
    @State private var expanded = true

    var body: some View {
        if let children = node.children {
            DisclosureGroup(isExpanded: $expanded) {
                FileTreeRows(nodes: children)
            } label: {
                Label(node.name, systemImage: "folder")
            }
        } else if let file = node.file {
            Label {
                VStack(alignment: .leading, spacing: 1) {
                    Text(node.name)
                    Text(file.kind)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            } icon: {
                Image(systemName: "doc")
            }
            .tag(file.path)
        }
    }
}

struct DiffView: View {
    let diff: GuiDiff

    var body: some View {
        if diff.isBinary {
            Text("Binary file")
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                .padding()
        } else {
            GeometryReader { geo in
                ScrollView([.horizontal, .vertical]) {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(Array(diff.lines.enumerated()), id: \.offset) { _, line in
                            Text(displayText(for: line))
                                .font(.system(.body, design: .monospaced))
                                .foregroundStyle(color(for: line.kind))
                                .frame(maxWidth: .infinity, alignment: .leading)
                        }
                    }
                    .frame(
                        minWidth: geo.size.width,
                        minHeight: geo.size.height,
                        alignment: .topLeading
                    )
                }
            }
        }
    }

    private func displayText(for line: GuiDiffLine) -> String {
        if line.kind == "hunk" {
            return line.content
        }
        return line.content
    }

    private func color(for kind: String) -> Color {
        switch kind {
        case "addition":
            return .green
        case "deletion":
            return .red
        case "hunk":
            return .cyan
        default:
            return .primary
        }
    }
}
