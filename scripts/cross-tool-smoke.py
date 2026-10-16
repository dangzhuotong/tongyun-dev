import tempfile,pathlib,subprocess,json,os,socket,sys
import argparse
parser=argparse.ArgumentParser(description='Isolated Dev -> API -> Git trace smoke test. Build both Rust CLIs first.')
parser.add_argument('--workspace',type=pathlib.Path,default=pathlib.Path(__file__).resolve().parents[2],help='Directory containing tongyun-dev, tongyun-api and tongyun-git')
base=parser.parse_args().workspace.resolve()
dev=str(base/'tongyun-dev/target/debug/tongyun-dev');git_agent=str(base/'tongyun-git/target/debug/tongyun-git-agent');runner=str(base/'tongyun-api/scripts/runner.ts')
with tempfile.TemporaryDirectory(prefix='tongyun-cross-tool-') as temp:
 root=pathlib.Path(temp);repo=root/'repo';repo.mkdir();audit=root/'audit';task='cross-tool-fixture'
 fixture=root/'fixture_server.py';fixture.write_text('from http.server import BaseHTTPRequestHandler,HTTPServer\nimport sys\nclass H(BaseHTTPRequestHandler):\n def log_message(self,*args): pass\n def do_GET(self):\n  self.send_response(200);self.send_header("Content-Type","application/json");self.end_headers();self.wfile.write(b\'{"fixture":true}\')\nHTTPServer(("127.0.0.1",int(sys.argv[1])),H).serve_forever()\n')
 with socket.socket() as sock:sock.bind(('127.0.0.1',0));port=sock.getsockname()[1]
 config=root/'projects.toml';config.write_text('log_dir = '+json.dumps(str(root/'logs'))+'\n[projects.fixture]\nroot = '+json.dumps(str(root))+'\nname = "Fixture"\nstart = ["'+sys.executable+'",'+json.dumps(str(fixture))+',"'+str(port)+'"]\nport = '+str(port)+'\nport_mode = "none"\nbackground = true\nauto_install = false\nhealth_url = "http://127.0.0.1:'+str(port)+'/health"\n')
 env=dict(os.environ);env.update({'HOME':temp,'GIT_CONFIG_GLOBAL':'/dev/null','GIT_CONFIG_NOSYSTEM':'1'})
 def invoke(args,structured=True):
  r=subprocess.run(args,cwd=repo,env=env,capture_output=True,text=True,timeout=25)
  if r.returncode:raise RuntimeError('fixture command failed: '+r.stderr[-300:]+r.stdout[-300:])
  return json.loads(r.stdout) if structured else r.stdout
 d=[dev,'--config',str(config),'--task',task,'--actor','agent:fixture']
 g=[git_agent,'--repo',str(repo),'--audit-dir',str(audit),'--task',task,'--actor','agent:fixture']
 started=False
 try:
  invoke(d+['list','--json']);launch=invoke(d+['start','fixture','--timeout','5','--json']);started=True;assert launch['ok']
  request=repo/'fixture.http';request.write_text('GET http://127.0.0.1:'+str(port)+'/fixture\n')
  api=invoke(['node','--experimental-strip-types',runner,str(request),'--root',str(repo),'--audit-dir',str(audit),'--task',task,'--actor','agent:fixture','--json']);assert api['passed']==1
  for command in [['init'],['config','user.name','Fixture'],['config','user.email','fixture@example.com'],['add','fixture.http']]:invoke(['git']+command,False)
  status=invoke(g+['status']);message=root/'commit-message';message.write_text('test: cross tool fixture')
  commit=invoke(g+['commit','--allow-write','--confirm',status['data']['fingerprint'],'--message-file',str(message)]);assert commit['ok']
  stop=invoke(d+['stop','fixture','--json']);started=False;assert stop['ok'] and not stop['status']['running']
  events=[json.loads(p.read_text()) for p in audit.glob('*.json')];tools={event['tool'] for event in events};assert tools=={'dev','api','git'};assert all(event['taskId']==task for event in events)
  phases={}
  for event in events:phases.setdefault(event['operationId'],set()).add(event['phase'])
  assert all(p=={'started','finished'} for p in phases.values())
  report={'passed':True,'scope':'isolated local fixtures; real Dev process lifecycle, HTTP Runner and Git commit','tools':sorted(tools),'operations':len(phases),'allFinished':True,'serviceStopped':True,'sharedTaskId':task}
  print(json.dumps(report))
 finally:
  if started:invoke(d+['stop','fixture','--json'])
