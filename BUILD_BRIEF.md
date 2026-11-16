# Build: Universal Computer Use MCP

## Objective

Build a lightweight, advanced, cross-platform Computer Use MCP server that gives existing AI coding agents the ability to intelligently operate the user's entire computer.

The MCP must work with:
- Google Antigravity
- Gemini CLI
- Claude Code
- OpenAI Codex
- Other MCP-compatible AI agents

This is NOT a new AI agent, chatbot, browser automation framework, or orchestration platform.

The existing AI agent provides all reasoning, planning, intelligence and decision-making. Our system provides the computer interaction capabilities.

## 1. Research before implementation

Do not reinvent existing technologies.

First investigate these projects:

1. Microsoft Hybrid Computer Use: https://github.com/microsoft/Hybrid-Computer-Use
2. Munim Computer Use: https://github.com/munim-ai/munim
3. a11y-computer-use: https://github.com/a11y-computer-use/a11y-computer-use
4. open-computer-use: https://github.com/open-computer-use/open-computer-use

Verify the actual repositories, documentation, licenses, implementation languages, platform support, dependencies and current maintenance status.

Compare their:
- Accessibility-tree implementations.
- UI element indexing and reference management.
- Native mouse and keyboard drivers.
- Screenshot capture.
- Visual targeting and OCR.
- MCP integration.
- Browser and application support.
- Resource consumption.

Select the best existing implementation as the foundation. Reuse libraries and components where their licenses permit.

Do not copy entire projects blindly. Avoid unnecessary dependencies and duplicate implementations.

Produce a short architecture decision document explaining the selected foundation and rejected alternatives.

## 2. Core architecture

Build a local-first MCP server.

Architecture:

AI Agent → MCP Server → Computer Interaction Runtime → Native OS APIs → Desktop Applications

The system must be model-agnostic.

It must not:
- Require its own LLM.
- Require cloud infrastructure.
- Depend on a specific AI provider.
- Replace the agent's existing terminal or filesystem tools.
- Require a separate autonomous orchestration framework.

## 3. Computer interaction capabilities

### A. UI discovery and indexing

This is a core feature.

The system should inspect the active application and discover interactive elements.

Extract:
- Element ID.
- Role.
- Accessible name.
- Description.
- Current value.
- Enabled/disabled state.
- Focus state.
- Bounding rectangle.
- Parent-child relationships.
- Available actions.

Example:

{
  "window": "WhatsApp",
  "elements": [
    {
      "id": "e1",
      "role": "textbox",
      "name": "Search",
      "enabled": true
    },
    {
      "id": "e2",
      "role": "button",
      "name": "New chat",
      "enabled": true
    },
    {
      "id": "e3",
      "role": "textbox",
      "name": "Message",
      "enabled": true
    },
    {
      "id": "e4",
      "role": "button",
      "name": "Send",
      "enabled": true
    }
  ]
}

The AI should be able to reference elements by ID rather than guessing screen coordinates.

Element IDs must be scoped to a UI snapshot or session. Validate references before executing actions and invalidate stale references when the UI changes.

Support compact, pruned accessibility trees instead of returning enormous raw trees to the model.

### B. Semantic interaction

Support:
- Click element.
- Double click.
- Right click.
- Type text.
- Replace text.
- Press keyboard shortcuts.
- Select dropdown options.
- Check/uncheck controls.
- Scroll elements.
- Drag and drop.
- Focus elements.
- Navigate UI hierarchies.

### C. Visual interaction

Accessibility APIs will not expose every application correctly.

Provide fallback capabilities:
- Capture screenshots.
- Inspect visible windows.
- OCR text detection.
- Visual element targeting.
- Coordinate-based clicks.
- Coordinate-based typing and scrolling.
- Screenshot comparison.

Use visual interaction only when semantic interaction is unavailable or insufficient.

### D. Application management

Support:
- List running applications.
- List application windows.
- Activate an application.
- Switch between windows.
- Inspect active window.
- Open applications.
- Detect dialogs and permission prompts.

### E. Feedback and verification

Every interaction must have a result.

After an action:
1. Execute the requested operation.
2. Detect whether the UI changed.
3. Return relevant updated state.
4. Report errors or unexpected results.
5. Allow the AI agent to inspect and retry.

Do not claim an action succeeded merely because an input event was sent.

## 4. MCP interface

Keep the public MCP interface small and composable.

Start with two primary tools:

### computer

For semantic computer operations:
- inspect
- get_ui_tree
- get_interactive_elements
- click
- type
- press_key
- scroll
- drag
- select
- focus

### desktop

For visual and application-level operations:
- screenshot
- list_applications
- list_windows
- activate_application
- visual_click
- visual_scroll
- window_information

You may adjust the final schema based on the selected existing implementation. Do not create dozens of redundant MCP tools.

Use structured arguments and structured responses. Include useful errors and execution metadata.

## 5. Operating system support

Design an abstraction layer for:

macOS:
- Accessibility API / AXUIElement.
- Native screenshot capture.
- Native input events.
- Window and application discovery.

Windows:
- Microsoft UI Automation.
- Native screenshot and input APIs.

Linux:
- AT-SPI.
- Appropriate native screenshot and input APIs.

Implement and thoroughly test macOS first.

Do not pretend cross-platform support is complete if only macOS works. Clearly document unsupported features and platform-specific limitations.

## 6. Browser and application support

This must not be a browser-only tool.

Support native desktop applications and browsers.

For browsers, investigate whether accessibility-based interaction and existing browser integrations can be reused.

Avoid creating a separate Playwright-based browser automation system unless a specific feature genuinely requires it.

Prefer native application interaction, semantic UI targeting and existing APIs when available.

## 7. Lightweight requirements

Optimize for:
- Low CPU and memory usage.
- Fast tool responses.
- Small accessibility-tree payloads.
- No unnecessary background polling.
- No mandatory cloud services.
- No mandatory model inference.
- No redundant screenshots.
- Minimal dependencies.
- Local execution.

Capture screenshots on demand rather than continuously.

## 8. Security

The system has potentially powerful access to the user's computer.

Implement:
- Local-only operation by default.
- Explicit permissions.
- Sensitive action confirmation.
- No credential extraction.
- No password logging.
- No arbitrary remote access.
- Clear action logs.
- Safe handling of destructive actions.

Require confirmation for sending messages, deleting files, purchases, financial transactions and other consequential operations.

## 9. Agent integration

Make installation straightforward.

Provide:
- MCP configuration for Antigravity.
- MCP configuration for Claude Code.
- MCP configuration for Gemini CLI.
- MCP configuration for Codex.
- A reusable SKILL.md explaining when and how agents should use the tools.

The skill must instruct agents to:
1. Inspect the current UI.
2. Prefer semantic element interaction.
3. Use visual fallback when necessary.
4. Inspect updated state after important actions.
5. Recover from errors.
6. Verify task completion.
7. Request approval for consequential actions.

Do not assume every agent supports slash commands in the same way. Provide the appropriate integration for each.

## 10. Testing and benchmarking

Build tests for:
- Element discovery.
- Element reference validity.
- Clicking and typing.
- Keyboard shortcuts.
- Application switching.
- Window detection.
- Screenshot fallback.
- UI state changes.
- Error recovery.
- Concurrent MCP clients.

Create a benchmark with real multi-step desktop workflows.

Compare:
- Accessibility-first interaction.
- Screenshot-only interaction.
- Hybrid interaction.

Measure:
- Task success rate.
- Action latency.
- Token consumption.
- CPU and memory usage.
- Recovery rate.

## 11. Development workflow

Work autonomously through the implementation.

Follow this order:

1. Inspect the existing repository and development environment.
2. Research existing projects.
3. Select and document the foundation.
4. Design the minimal architecture.
5. Implement the macOS MVP.
6. Connect it to Antigravity.
7. Create the reusable skill.
8. Test actual desktop workflows.
9. Optimize performance.
10. Prepare documentation and installation instructions.

Do not stop after producing a plan or architecture document. Actually build and test the working implementation.

Avoid unnecessary abstractions, premature multi-agent orchestration and rebuilding existing functionality.

If an external dependency or API is unavailable, document it and find a practical alternative.

## Definition of done

A user should be able to install the MCP once and connect it to Antigravity, Claude Code, Gemini CLI and Codex.

Each agent should be able to:
- Discover UI elements.
- Reference indexed elements.
- Operate native applications.
- Interact with browsers.
- Use visual fallback.
- Verify actions.
- Complete multi-step desktop workflows.

The result should be a lightweight, reliable and reusable computer-use infrastructure that gives existing AI agents hands and eyes without replacing their intelligence.