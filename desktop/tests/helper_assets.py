"""Native Ubuntu crash/noexec/ownership acceptance. No global mount changes."""
import hashlib, json, os, pathlib, secrets, select, subprocess, sys, tempfile, time, urllib.request

source = pathlib.Path(sys.argv[1])
assets = (source / 'desktop/src/helper_assets.py').read_text()
bootstrap = (source / 'desktop/src/bootstrap.py').read_text()
binary = (source / 'desktop/assets/worker-linux-x86_64').read_bytes()
digest = hashlib.sha256(binary).hexdigest()
report = dict(status='FAIL', checks=[], workerSha256=digest)
namespace = {'__name__':'helper_assets_test'}
exec(compile(assets, 'helper_assets.py', 'exec'), namespace)

def until(check, seconds=12):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        if check():
            return
        time.sleep(.05)
    raise AssertionError('Deadline: '+str(check))

with tempfile.TemporaryDirectory(prefix='branchline-d9-helper-') as fixture:
    root = pathlib.Path(fixture)
    (root/'tmp').mkdir()
    state = root/'state'
    env = dict(os.environ, TMPDIR=str(root/'tmp'), XDG_STATE_HOME=str(state), XDG_CONFIG_HOME=str(root/'config'))
    os.environ['XDG_STATE_HOME'] = str(state)
    receipts = state/'branchline/helper-assets-v1'
    def recover():
        fd = namespace['ledger']()
        try:
            namespace['recover'](fd)
        finally:
            os.close(fd)
    def records():
        return list(receipts.glob('*.json'))
    def frame(data=binary, sha=None):
        return (json.dumps(dict(size=len(data),sha256=sha or hashlib.sha256(data).hexdigest()))+'\n').encode()+data
    def launch(patch='', custom_env=None):
        return subprocess.Popen([sys.executable,'-c',assets+'\n'+patch+'\n'+bootstrap],
                                stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=custom_env or env)
    def ready(process):
        assert select.select([process.stdout],[],[],15)[0], 'startup timeout'
        result = json.loads(process.stdout.readline())
        assert 'port' in result, result
        return result
    def start(patch=''):
        process = launch(patch)
        token = secrets.token_hex(32)
        process.stdin.write(frame()+(json.dumps(dict(token=token,nonce=secrets.token_hex(32),protocol=1))+'\n').encode())
        process.stdin.flush()
        return process,ready(process),token
    def handshake(ready, token):
        request = urllib.request.Request('http://127.0.0.1:%s/api/v1/runtime/handshake'%ready['port'],headers={'Authorization':'Bearer '+token})
        with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(request,timeout=2) as response:
            assert response.status == 200
    def crashed_asset():
        process=launch('import subprocess\nsubprocess.Popen=lambda *a,**k: os._exit(91)')
        process.communicate(frame(),timeout=15)
        assert process.returncode == 91
        record=records()[-1]
        data=json.loads(record.read_text())
        return record,pathlib.Path(data['parent'])/data['name']

    process,connection,token=start()
    handshake(connection,token)
    process.stdin.close();process.wait(timeout=12)
    assert process.returncode == 0 and records()==[] and list((root/'tmp').iterdir())==[]
    report['checks'].append('normal bootstrap/authenticated worker/lease cleanup PASS')

    # Distro metadata must never gate a binary-compatible environment. These are
    # metadata/ABI boundary simulations running the real native worker, not claims
    # of a full separate distro installation.
    for name,patch in [
        ('Ubuntu 24.04 / glibc 2.39',"platform.freedesktop_os_release=lambda:dict(ID='ubuntu',VERSION_ID='24.04')\nplatform.libc_ver=lambda:('glibc','2.39')"),
        ('other distro / compatible GNU ABI',"platform.freedesktop_os_release=lambda:dict(ID='debian',VERSION_ID='13')"),
        ('unavailable distro metadata',"platform.freedesktop_os_release=lambda: (_ for _ in ()).throw(OSError('metadata unavailable'))")]:
        process,connection,token=start(patch)
        handshake(connection,token)
        process.stdin.close();process.wait(timeout=12)
        assert process.returncode==0 and records()==[]
        report['checks'].append('simulated '+name+': real worker auth/lease PASS')

    record,path=crashed_asset()
    assert path.is_dir()
    recover()
    assert not path.exists() and not record.exists()
    report['checks'].append('forced bootstrap exit after durable asset receipt, before worker spawn: next-start recovery PASS')

    # An identically named unknown directory and a credential sentinel have no receipt.
    unknown=root/'tmp'/('awm-runtime-v2-'+secrets.token_hex(16));unknown.mkdir();(unknown/'credentials').write_text('fixture-private-sentinel')
    external=subprocess.Popen(['/usr/bin/sleep','120'])
    process,connection,token=start()
    active_record=records()[0];data=json.loads(active_record.read_text());active=pathlib.Path(data['parent'])/data['name']
    process.kill();process.wait(timeout=5)  # Host still holds the worker's stdin pipe.
    recover();handshake(connection,token)
    assert active.exists() and active_record.exists() and external.poll() is None
    process.stdin.close()
    until(lambda: (recover() is None) and not active.exists())
    assert not active_record.exists()
    assert (unknown/'credentials').read_text()=='fixture-private-sentinel' and external.poll() is None
    external.terminate();external.wait(timeout=5)
    report['checks'].append('bootstrap SIGKILL: active other-session helper retained by inherited lease; EOF then recovery; unknown directory/credentials/external process preserved PASS')

    # Real host os._exit closes its private pipe. Bootstrap/helper must finish themselves.
    host_code='''import subprocess,sys,json,secrets,os
p=subprocess.Popen([sys.executable,'-c',sys.argv[1]],stdin=subprocess.PIPE,stdout=subprocess.PIPE,env=os.environ)
binary=open(sys.argv[2],'rb').read();import hashlib
p.stdin.write((json.dumps(dict(size=len(binary),sha256=hashlib.sha256(binary).hexdigest()))+'\\n').encode()+binary+(json.dumps(dict(token=secrets.token_hex(32),nonce=secrets.token_hex(32),protocol=1))+'\\n').encode());p.stdin.flush()
assert 'port' in json.loads(p.stdout.readline());os._exit(92)
'''
    host=subprocess.Popen([sys.executable,'-c',host_code,assets+'\n'+bootstrap,str(source/'desktop/assets/worker-linux-x86_64')],env=env)
    host.wait(timeout=20);assert host.returncode==92
    until(lambda: records()==[])
    report['checks'].append('forced host os._exit: stdin lease closes worker/bootstrap and owned assets PASS')

    record,modified=crashed_asset();worker=modified/'worker';content=worker.read_bytes();worker.write_bytes(b'X'+content[1:])
    recover();assert worker.exists() and record.exists()
    worker.write_bytes(content);(modified/'credentials').write_text('private')
    recover();assert (modified/'credentials').read_text()=='private' and worker.exists()
    (modified/'credentials').unlink();recover();assert not modified.exists()
    record,replaced=crashed_asset();moved=replaced.with_name(replaced.name+'-moved');replaced.rename(moved);replaced.mkdir();(replaced/'credentials').write_text('replacement-private')
    recover();assert (replaced/'credentials').read_text()=='replacement-private' and (moved/'worker').exists()
    # Test-owned restoration makes exact original identities provable again.
    (replaced/'credentials').unlink();replaced.rmdir();moved.rename(replaced);recover();assert not replaced.exists()
    forged=receipts/('awm-runtime-v2-'+secrets.token_hex(16)+'.json');forged.symlink_to(unknown/'credentials')
    recover();assert forged.is_symlink() and (unknown/'credentials').read_text()=='fixture-private-sentinel';forged.unlink()
    report['checks'].append('modified worker, extra credentials, directory inode replacement, symlink receipt all retained; exact restoration cleans owned assets PASS')

    negatives=[('bad digest','',frame(sha='0'*64),'helper_asset'),
               ('wrong ELF','',frame(b'not an ELF'),'helper_platform'),
               ('simulated ARM64',"platform.machine=lambda:'aarch64'",frame(),'helper_platform'),
               ('simulated incompatible glibc',"platform.libc_ver=lambda:('glibc','2.38')",frame(),'helper_platform'),
               ('simulated musl ABI',"platform.libc_ver=lambda:('musl','1.2')",frame(),'helper_platform'),
               ('simulated old Python',"sys.version_info=(3,9,0)",frame(),'helper_prerequisite'),
               ('simulated missing Git',"os.access=lambda *a:False",frame(),'helper_prerequisite')]
    for name,patch,data,expected in negatives:
        process=launch(patch);out,error=process.communicate(data,timeout=15)
        assert process.returncode!=0 and json.loads(out)['error']==expected,(name,out,error)
        report['checks'].append(name+': '+expected+' PASS')
    with tempfile.TemporaryDirectory(prefix='branchline-d9-noexec-',dir='/run/lock') as noexec:
        options=subprocess.check_output(['findmnt','-T',noexec,'-no','OPTIONS'],text=True)
        assert 'noexec' in options
        process=launch(custom_env=dict(env,TMPDIR=noexec));out,error=process.communicate(frame(),timeout=15)
        assert process.returncode!=0 and json.loads(out)['error']=='helper_temp',(out,error)
        assert list(pathlib.Path(noexec).iterdir())==[] and records()==[]
    report['checks'].append('native noexec /run/lock TMPDIR: explicit helper_temp refusal, assets/receipts cleaned PASS')
    report['status']='PASS'
    print(json.dumps(report,ensure_ascii=False))
