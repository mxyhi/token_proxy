//go:build darwin && cgo

package proc

/*
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <spawn.h>
#include <stdlib.h>
#include <sys/types.h>

// responsibility_spawnattrs_setdisclaim (libSystem, macOS 10.14 on) has
// the child answer for itself to TCC; looked up, as it has no header.
static int magpie_spawn_own(const char *path, char *const argv[], char *const envp[], const char *dir, int out, pid_t *pid) {
	posix_spawnattr_t attr;
	posix_spawn_file_actions_t fa;
	int err = posix_spawnattr_init(&attr);
	if (err != 0) return err;
	err = posix_spawn_file_actions_init(&fa);
	if (err != 0) { posix_spawnattr_destroy(&attr); return err; }
	sigset_t none, all;
	sigemptyset(&none);
	sigfillset(&all);
	posix_spawnattr_setsigmask(&attr, &none);
	posix_spawnattr_setsigdefault(&attr, &all);
	// only stdin, stdout and stderr are passed on, none of magpie's own
	posix_spawnattr_setflags(&attr, POSIX_SPAWN_CLOEXEC_DEFAULT | POSIX_SPAWN_SETSIGMASK | POSIX_SPAWN_SETSIGDEF);
	int (*disclaim)(posix_spawnattr_t *, int) = (int (*)(posix_spawnattr_t *, int))dlsym(RTLD_DEFAULT, "responsibility_spawnattrs_setdisclaim");
	if (disclaim != NULL) disclaim(&attr, 1);
	posix_spawn_file_actions_addopen(&fa, 0, "/dev/null", O_RDONLY, 0);
	posix_spawn_file_actions_adddup2(&fa, out, 1);
	posix_spawn_file_actions_adddup2(&fa, out, 2);
	// The macOS 26 SDK deprecates the _np call for
	// posix_spawn_file_actions_addchdir, which only macOS 26 has. magpie
	// runs on macOS 12 and later, where the _np call is always there, so it
	// stays, with the warning off for this line alone.
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
	if (dir != NULL) posix_spawn_file_actions_addchdir_np(&fa, dir);
#pragma clang diagnostic pop
	err = posix_spawn(pid, path, &fa, &attr, argv, envp);
	posix_spawn_file_actions_destroy(&fa);
	posix_spawnattr_destroy(&attr);
	return err;
}

static int magpie_responsible(pid_t pid) {
	pid_t (*f)(pid_t) = (pid_t (*)(pid_t))dlsym(RTLD_DEFAULT, "responsibility_get_pid_responsible_for_pid");
	if (f == NULL) return 0;
	pid_t r = f(pid);
	return r > 0 ? (int)r : 0;
}
*/
import "C"

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"os"
	"os/exec"
	"syscall"
	"time"
	"unsafe"
)

// cStrings is ss as a NULL-ended C array, and its freeing.
func cStrings(ss []string) (**C.char, func()) {
	arr := (**C.char)(C.malloc(C.size_t(len(ss)+1) * C.size_t(unsafe.Sizeof(uintptr(0)))))
	view := unsafe.Slice(arr, len(ss)+1)
	for i, s := range ss {
		view[i] = C.CString(s)
	}
	view[len(ss)] = nil
	return arr, func() {
		for i := range ss {
			C.free(unsafe.Pointer(view[i]))
		}
		C.free(unsafe.Pointer(arr))
	}
}

// spawnOwn starts cmd as its own responsible process, its stdout and
// stderr on out.
func spawnOwn(cmd *exec.Cmd, out *os.File) (int, error) {
	env := cmd.Env
	if env == nil {
		env = os.Environ()
	}
	path := C.CString(cmd.Path)
	defer C.free(unsafe.Pointer(path))
	argv, freeArgv := cStrings(cmd.Args)
	defer freeArgv()
	envp, freeEnvp := cStrings(env)
	defer freeEnvp()
	var dir *C.char
	if cmd.Dir != "" {
		dir = C.CString(cmd.Dir)
		defer C.free(unsafe.Pointer(dir))
	}
	var pid C.pid_t
	if e := C.magpie_spawn_own(path, argv, envp, dir, C.int(out.Fd()), &pid); e != 0 {
		return 0, &os.PathError{Op: "posix_spawn", Path: cmd.Path, Err: syscall.Errno(e)}
	}
	return int(pid), nil
}

func combinedOutputOwn(ctx context.Context, cmd *exec.Cmd) ([]byte, error) {
	r, w, err := os.Pipe()
	if err != nil {
		return nil, err
	}
	pid, err := spawnOwn(cmd, w)
	w.Close()
	if err != nil {
		r.Close()
		return nil, err
	}
	var buf bytes.Buffer
	read := make(chan struct{})
	go func() {
		io.Copy(&buf, r)
		close(read)
	}()
	type exit struct {
		ws  syscall.WaitStatus
		err error
	}
	waited := make(chan exit, 1)
	go func() {
		for {
			var ws syscall.WaitStatus
			_, err := syscall.Wait4(pid, &ws, 0, nil)
			if err != syscall.EINTR {
				waited <- exit{ws, err}
				return
			}
		}
	}()
	var x exit
	select {
	case x = <-waited:
	case <-ctx.Done():
		syscall.Kill(pid, syscall.SIGKILL)
		x = <-waited
	}
	// what it started may hold the output open; it is let go a moment
	// after, as CommandContext's WaitDelay does
	select {
	case <-read:
	case <-time.After(waitDelay):
		r.Close()
		<-read
	}
	r.Close()
	if err := ctx.Err(); err != nil {
		return buf.Bytes(), err
	}
	if x.err != nil {
		return buf.Bytes(), x.err
	}
	switch {
	case x.ws.Signaled():
		return buf.Bytes(), fmt.Errorf("signal: %v", x.ws.Signal())
	case x.ws.ExitStatus() != 0:
		return buf.Bytes(), fmt.Errorf("exit status %d", x.ws.ExitStatus())
	}
	return buf.Bytes(), nil
}

// Responsible is the process macOS holds responsible for pid, the one its
// privacy prompts name; 0 when that can't be told.
func Responsible(pid int) int { return int(C.magpie_responsible(C.pid_t(pid))) }
