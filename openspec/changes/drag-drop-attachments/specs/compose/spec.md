# Spec Delta

## ADDED Requirements

### Requirement: Files dropped on a New Message are attached
The system SHALL attach every regular file dropped anywhere on a New Message window,
including the message body, reading them without blocking the window.

#### Scenario: Dropping files from a file manager
- **WHEN** the user drags two files from the file manager onto the message body and drops them
- **THEN** both appear as attachments and the body text is unchanged

#### Scenario: Dropping a folder
- **WHEN** the user drops a folder
- **THEN** nothing is attached for it and a notice says folders can't be attached

#### Scenario: Dropping a web link
- **WHEN** the user drops a link dragged from a web browser
- **THEN** the link is inserted into the body as text and nothing is downloaded or attached

### Requirement: Drag feedback
While files are dragged over a New Message window the system SHALL show that dropping will
attach them.

#### Scenario: Hovering with files
- **WHEN** files are dragged over the window
- **THEN** the window shows an outline and "Drop to attach" until the drag leaves or drops

### Requirement: Attachment sizes are visible
The system SHALL show each attachment's size and warn when the total exceeds 25 MB.

#### Scenario: Large attachments
- **WHEN** the attachments add up to more than 25 MB
- **THEN** a notice warns that many mail servers reject messages that large
