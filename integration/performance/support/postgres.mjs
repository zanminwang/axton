// pg Pool.end() can resolve after removing clients from its list but before
// their sockets emit 'end'. A database must not be force-dropped in that gap.
export function poolLifecycle(pool, onError) {
  const endings=new Set();
  pool.on('error',onError);
  pool.on('connect',client=>{
    const ended=new Promise(resolve=>client.once('end',resolve));
    endings.add(ended);
    void ended.then(()=>endings.delete(ended));
  });
  let closing;
  return () => closing??=(async()=>{
    await pool.end();
    await Promise.all([...endings]);
  })();
}
