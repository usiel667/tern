# Spec Delta

## ADDED Requirements

### Requirement: Sent messages can be taken back for a few seconds
When the user sends a message, the system SHALL hold it for 10 seconds before sending it and
show a notice with an Undo button for that time. Undo SHALL stop the message from being sent
and reopen it, as it was, in a New Message window, saved to Drafts again.

#### Scenario: Undo
- **WHEN** the user sends a message and presses Undo within 10 seconds
- **THEN** the message is not sent, and its window reopens with the same recipients, subject, text and attachments

#### Scenario: No undo
- **WHEN** the user sends a message and does nothing
- **THEN** it is sent about 10 seconds later, and replying still marks the original as answered

#### Scenario: Quit during the hold
- **WHEN** Tern quits or crashes while a message is held
- **THEN** the message is not lost: it is sent the next time Tern starts
