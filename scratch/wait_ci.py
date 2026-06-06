import urllib.request
import json
import time
import sys

url = 'https://api.github.com/repos/namefailed/phoneme/actions/runs'

print("Waiting for CI run to start...")
time.sleep(10)

while True:
    try:
        req = urllib.request.Request(url)
        with urllib.request.urlopen(req) as response:
            data = json.loads(response.read().decode())
            run = data['workflow_runs'][0]
            status = run['status']
            conclusion = run['conclusion']
            commit_msg = run['head_commit']['message']
            print(f"Run {run['id']} ({commit_msg[:30]}...): status={status}, conclusion={conclusion}")
            if status == 'completed':
                if conclusion == 'success':
                    print("CI SUCCESS!")
                    sys.exit(0)
                else:
                    print(f"CI FAILED! Conclusion: {conclusion}")
                    sys.exit(1)
    except Exception as e:
        print(f"Error: {e}")
    time.sleep(15)
