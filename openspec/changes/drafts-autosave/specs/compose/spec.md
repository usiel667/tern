# Spec Delta

## ADDED Requirements

### Requirement: Messages being written are saved as drafts
While a New Message window has content, the system SHALL save it as a draft a few seconds
after typing stops and at least every 30 seconds while typing continues, and when the user
presses Ctrl+S. Each save SHALL be kept on this computer immediately and uploaded to the
account's Drafts mailbox, replacing the previous server copy.

#### Scenario: Autosave while writing
- **WHEN** the user types a message and pauses
- **THEN** within a few seconds the message appears in the account's Drafts mailbox, once

#### Scenario: Offline
- **WHEN** the user writes a message while offline
- **THEN** the draft is kept on this computer and uploaded to Drafts when the account is reachable again

#### Scenario: Bcc in drafts
- **WHEN** a draft with a Bcc recipient is reopened
- **THEN** the Bcc recipient is still there

### Requirement: Closing a message offers to keep it
When the user closes a New Message window that has content, the system SHALL ask whether to
save it as a draft: Save keeps the draft, Don't Save removes any saved draft, Cancel returns
to the message. A window without content SHALL close without asking.

#### Scenario: Don't Save after autosave
- **WHEN** a message was autosaved and the user closes it and chooses Don't Save
- **THEN** the draft is removed from Drafts

### Requirement: Sending removes the draft
When a message that was saved as a draft is sent, the system SHALL remove its draft.

#### Scenario: Send after autosave
- **WHEN** the user sends a message that had been autosaved
- **THEN** the message is sent and no copy is left in Drafts

### Requirement: Drafts can be edited
The system SHALL open a message from a Drafts mailbox for editing (Edit Draft in the viewer,
or double-click/Return in the list), with its recipients including Bcc, subject, text and
attachments, and later saves SHALL replace that server copy.

#### Scenario: Draft written elsewhere
- **WHEN** the user opens a draft that was written on their phone
- **THEN** it opens in a New Message window, and saving it replaces the phone's copy instead of adding a second one
