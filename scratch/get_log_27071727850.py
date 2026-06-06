import urllib.request
import json
import zipfile
import io
import sys

run_id = '27071727850'
url = f'https://api.github.com/repos/namefailed/phoneme/actions/runs/{run_id}/logs'
req = urllib.request.Request(url)
try:
    with urllib.request.urlopen(req) as response:
        with zipfile.ZipFile(io.BytesIO(response.read())) as thezip:
            for zipinfo in thezip.infolist():
                if 'Test' in zipinfo.filename or 'Clippy' in zipinfo.filename or 'Tauri build' in zipinfo.filename:
                    print(f"--- {zipinfo.filename} ---")
                    lines = thezip.read(zipinfo).decode('utf-8').splitlines()
                    for line in lines:
                        if 'error' in line.lower() or 'fail' in line.lower():
                            print(line)
except Exception as e:
    print(f"Error fetching logs: {e}")
