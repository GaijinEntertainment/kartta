package main

type file struct{}

func (file) close() {}

type conn struct{}

func (conn) close() {}

func shut(f file, open func() conn) {
	f.close()
	open().close()
}
