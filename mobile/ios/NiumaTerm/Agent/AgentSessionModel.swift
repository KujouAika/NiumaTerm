import SwiftUI
import Observation

/// Mirrors the slots of the replicated `AgentView` (design doc §8.3).
/// Real app: fed by `AgentHandle` / `AgentObserver`; actions become `agent.call` commands.
@MainActor
@Observable
final class AgentSessionModel {
    let session: Session
    var title: String

    // transcript
    var entries: [TranscriptEntry]
    // status slot
    var isWorking: Bool
    var interrupted = false
    var workStarted: Date
    var tokens = "6.5k"
    // composer
    var draft = ""
    var queue: [QueuedPrompt] = []
    // settings slot
    var model: String
    var effort = "Medium"
    let efforts = ["Low", "Medium", "High"]
    // pending slot
    var pending: ApprovalRequest?
    var showApproval = false
    var allowForSession = false
    // handover (§8.4)
    var takenBack = false
    // footer
    var branch = "feature/remote-session"
    var contextLine = "80k used · 92% left"

    @ObservationIgnored private var nextID: Int
    @ObservationIgnored private var replyTask: Task<Void, Never>?

    init(session: Session) {
        self.session = session
        self.title = session.title
        let seed = MockData.agentSeed(for: session)
        self.entries = seed.entries
        self.isWorking = seed.working
        self.workStarted = Date().addingTimeInterval(-seed.elapsed)
        self.pending = seed.approval
        self.model = session.profile?.models.first ?? "default"
        self.nextID = (seed.entries.map(\.id).max() ?? -1) + 1
        self.showApproval = seed.approval != nil
    }

    var profile: AgentProfile { session.profile ?? .claude }
    var agentName: String { profile.shortName }
    var trimmedDraft: String { draft.trimmingCharacters(in: .whitespacesAndNewlines) }
    var showsStop: Bool { isWorking && trimmedDraft.isEmpty }
    var primaryEnabled: Bool { isWorking || !trimmedDraft.isEmpty }

    /// Send, queue (while working), or interrupt.
    func primaryAction() {
        let text = trimmedDraft
        if !text.isEmpty {
            draft = ""
            if isWorking {
                queue.append(QueuedPrompt(text: text))
            } else {
                send(text)
            }
        } else if isWorking {
            interrupt()
        }
    }

    func withdraw(_ q: QueuedPrompt) {
        queue.removeAll { $0.id == q.id }
    }

    func interrupt() {
        replyTask?.cancel()
        isWorking = false
        interrupted = true
    }

    func approve() {
        showApproval = false
        pending = nil
        removeWaitingNotice()
        startTurn {
            [.tools([ToolCall("Bash", "git commit -m …")]), .agent("Committed.")]
        }
    }

    func deny() {
        showApproval = false
        pending = nil
        removeWaitingNotice()
        append(.notice("✕ Denied", Theme.secondary))
        append(.agent("Understood. I left the changes staged and did not commit."))
    }

    func simulateApproval() {
        pending = ApprovalRequest(title: "\(agentName) needs approval",
                                  summary: "Run a command in \(session.cwd.split(separator: "\\").last.map(String.init) ?? "workspace")",
                                  command: "cargo test -p nmt_remote",
                                  cwd: session.cwd,
                                  ruleLabel: "Allow cargo test for this session")
        append(.notice("◌ Waiting for approval", Theme.attention))
        showApproval = true
    }

    private func send(_ text: String) {
        append(.user(text))
        startTurn {
            [.tools([ToolCall("Read", "AGENTS.md"), ToolCall("Grep", "remote")]),
             .agent("(Demo) Reply from the mock core. Swap `AgentSessionModel` onto `AgentHandle` to drive a real host.")]
        }
    }

    private func startTurn(_ steps: @escaping () -> [TranscriptEntry.Kind]) {
        replyTask?.cancel()
        interrupted = false
        isWorking = true
        workStarted = .now
        replyTask = Task { [weak self] in
            for step in steps() {
                try? await Task.sleep(for: .seconds(1.3))
                guard let self, !Task.isCancelled else { return }
                self.append(step)
            }
            guard let self, !Task.isCancelled else { return }
            self.finishTurn()
        }
    }

    private func finishTurn() {
        isWorking = false
        if !queue.isEmpty {
            let next = queue.removeFirst()
            send(next.text)
        }
    }

    private func removeWaitingNotice() {
        entries.removeAll {
            if case .notice(let t, _) = $0.kind { return t.hasPrefix("◌") }
            return false
        }
    }

    private func append(_ kind: TranscriptEntry.Kind) {
        entries.append(TranscriptEntry(id: nextID, kind: kind))
        nextID += 1
    }
}
