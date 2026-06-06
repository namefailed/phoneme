import os

def replace_in_file(filepath):
    with open(filepath, 'r', encoding='utf-8') as f:
        content = f.read()

    new_content = content.replace('session_id', 'meeting_id')
    new_content = new_content.replace('session_name', 'meeting_name')
    new_content = new_content.replace('SessionId', 'MeetingId')
    new_content = new_content.replace('SessionName', 'MeetingName')
    new_content = new_content.replace('session_id:', 'meeting_id:')
    new_content = new_content.replace('sessionName', 'meetingName')
    new_content = new_content.replace('sessionId', 'meetingId')
    new_content = new_content.replace('Session name', 'Meeting name')
    new_content = new_content.replace('session name', 'meeting name')
    new_content = new_content.replace('ListSession', 'ListMeeting')
    new_content = new_content.replace('list_session', 'list_meeting')
    new_content = new_content.replace('list_by_session', 'list_by_meeting')
    new_content = new_content.replace('update_session_name', 'update_meeting_name')

    if new_content != content:
        with open(filepath, 'w', encoding='utf-8') as f:
            f.write(new_content)
        print(f"Updated {filepath}")

for root, dirs, files in os.walk('.'):
    # Exclude node_modules, target, .git
    if 'node_modules' in dirs:
        dirs.remove('node_modules')
    if 'target' in dirs:
        dirs.remove('target')
    if '.git' in dirs:
        dirs.remove('.git')
    if 'dist' in dirs:
        dirs.remove('dist')

    for file in files:
        if file.endswith('.rs') or file.endswith('.ts') or file.endswith('.md') or file.endswith('.json') or file.endswith('.html'):
            replace_in_file(os.path.join(root, file))
