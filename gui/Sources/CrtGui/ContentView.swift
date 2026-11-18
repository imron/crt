import SwiftUI

struct ContentView: View {
    @ObservedObject var store: GuiStore
    @State private var selection: String?

    var body: some View {
        NavigationSplitView {
            fileList
        } detail: {
            diffPane
        }
        .frame(minWidth: 800, minHeight: 480)
        .onAppear {
            selection = store.snapshot.selectedPath
        }
        .onChange(of: store.snapshot.selectedPath) { _, path in
            selection = path
        }
        .onChange(of: selection) { _, path in
            guard let path, path != store.snapshot.selectedPath else {
                return
            }
            store.selectFile(path)
        }
    }

    private var fileList: some View {
        List(store.snapshot.files, selection: $selection) { file in
            VStack(alignment: .leading, spacing: 2) {
                Text(file.path)
                    .font(.system(.body, design: .monospaced))
                Text("\(file.status) · \(file.kind)")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            .tag(file.path)
        }
        .listStyle(.sidebar)
        .navigationTitle(store.snapshot.repoName)
    }

    @ViewBuilder
    private var diffPane: some View {
        if let diff = store.snapshot.diff {
            DiffView(diff: diff)
        } else {
            ContentUnavailableView(
                "No file selected",
                systemImage: "doc.text",
                description: Text("Choose a file from the sidebar.")
            )
        }
    }
}

struct DiffView: View {
    let diff: GuiDiff

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(diff.path)
                .font(.headline)
                .padding(8)
            Divider()
            if diff.isBinary {
                Text("Binary file")
                    .foregroundStyle(.secondary)
                    .padding()
            } else {
                ScrollView([.horizontal, .vertical]) {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(Array(diff.lines.enumerated()), id: \.offset) { _, line in
                            Text(line.content)
                                .font(.system(.body, design: .monospaced))
                                .foregroundStyle(color(for: line.kind))
                                .frame(maxWidth: .infinity, alignment: .leading)
                        }
                    }
                    .padding(8)
                }
            }
        }
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
