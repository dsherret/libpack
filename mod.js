/// <reference types="./mod.d.ts" />
import * as pack6 from "https://deno.land/x/deno_cache@0.4.1/mod.ts";
import * as pack4 from "https://deno.land/x/wasmbuild@0.14.1/loader.ts";
import * as pack5 from "https://deno.land/x/wasmbuild@0.14.1/cache.ts";
import * as pack1 from "https://deno.land/std@0.191.0/path/mod.ts";
const pack0 = {
  instantiate: undefined,
  instantiateWithInstance: undefined,
  isInstantiated: undefined,
  pack: undefined
};
const pack3 = {
  fetch_specifier: undefined
};
(function _lib_snippets_rs_lib_aa8c88480f363a4a_helpers_js() {
  const fileFetcher = pack6.createCache();
  function fetch_specifier(specifier) {
    return fileFetcher.load(new URL(specifier));
  }
  Object.defineProperty(pack3, "fetch_specifier", {
    get: ()=>fetch_specifier
  });
})();
(function _lib_rs_lib_generated_js() {
  // @generated file from wasmbuild -- do not edit
  // deno-lint-ignore-file
  // deno-fmt-ignore-file
  // source-hash: fc6ddbbd69b2d1d7dd3fae979660dfe9929f8597
  let wasm;
  const heap = new Array(128).fill(undefined);
  heap.push(undefined, null, true, false);
  function getObject(idx) {
    return heap[idx];
  }
  function isLikeNone(x) {
    return x === undefined || x === null;
  }
  let cachedFloat64Memory0 = null;
  function getFloat64Memory0() {
    if (cachedFloat64Memory0 === null || cachedFloat64Memory0.byteLength === 0) {
      cachedFloat64Memory0 = new Float64Array(wasm.memory.buffer);
    }
    return cachedFloat64Memory0;
  }
  let cachedInt32Memory0 = null;
  function getInt32Memory0() {
    if (cachedInt32Memory0 === null || cachedInt32Memory0.byteLength === 0) {
      cachedInt32Memory0 = new Int32Array(wasm.memory.buffer);
    }
    return cachedInt32Memory0;
  }
  let WASM_VECTOR_LEN = 0;
  let cachedUint8Memory0 = null;
  function getUint8Memory0() {
    if (cachedUint8Memory0 === null || cachedUint8Memory0.byteLength === 0) {
      cachedUint8Memory0 = new Uint8Array(wasm.memory.buffer);
    }
    return cachedUint8Memory0;
  }
  const cachedTextEncoder = typeof TextEncoder !== "undefined" ? new TextEncoder("utf-8") : {
    encode: ()=>{
      throw Error("TextEncoder not available");
    }
  };
  const encodeString = function(arg, view) {
    return cachedTextEncoder.encodeInto(arg, view);
  };
  function passStringToWasm0(arg, malloc, realloc) {
    if (realloc === undefined) {
      const buf = cachedTextEncoder.encode(arg);
      const ptr = malloc(buf.length) >>> 0;
      getUint8Memory0().subarray(ptr, ptr + buf.length).set(buf);
      WASM_VECTOR_LEN = buf.length;
      return ptr;
    }
    let len = arg.length;
    let ptr = malloc(len) >>> 0;
    const mem = getUint8Memory0();
    let offset = 0;
    for(; offset < len; offset++){
      const code = arg.charCodeAt(offset);
      if (code > 0x7F) break;
      mem[ptr + offset] = code;
    }
    if (offset !== len) {
      if (offset !== 0) {
        arg = arg.slice(offset);
      }
      ptr = realloc(ptr, len, len = offset + arg.length * 3) >>> 0;
      const view = getUint8Memory0().subarray(ptr + offset, ptr + len);
      const ret = encodeString(arg, view);
      offset += ret.written;
    }
    WASM_VECTOR_LEN = offset;
    return ptr;
  }
  let heap_next = heap.length;
  function addHeapObject(obj) {
    if (heap_next === heap.length) heap.push(heap.length + 1);
    const idx = heap_next;
    heap_next = heap[idx];
    heap[idx] = obj;
    return idx;
  }
  const cachedTextDecoder = typeof TextDecoder !== "undefined" ? new TextDecoder("utf-8", {
    ignoreBOM: true,
    fatal: true
  }) : {
    decode: ()=>{
      throw Error("TextDecoder not available");
    }
  };
  if (typeof TextDecoder !== "undefined") cachedTextDecoder.decode();
  function getStringFromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return cachedTextDecoder.decode(getUint8Memory0().subarray(ptr, ptr + len));
  }
  function dropObject(idx) {
    if (idx < 132) return;
    heap[idx] = heap_next;
    heap_next = idx;
  }
  function takeObject(idx) {
    const ret = getObject(idx);
    dropObject(idx);
    return ret;
  }
  let cachedBigInt64Memory0 = null;
  function getBigInt64Memory0() {
    if (cachedBigInt64Memory0 === null || cachedBigInt64Memory0.byteLength === 0) {
      cachedBigInt64Memory0 = new BigInt64Array(wasm.memory.buffer);
    }
    return cachedBigInt64Memory0;
  }
  function debugString(val) {
    // primitive types
    const type = typeof val;
    if (type == "number" || type == "boolean" || val == null) {
      return `${val}`;
    }
    if (type == "string") {
      return `"${val}"`;
    }
    if (type == "symbol") {
      const description = val.description;
      if (description == null) {
        return "Symbol";
      } else {
        return `Symbol(${description})`;
      }
    }
    if (type == "function") {
      const name = val.name;
      if (typeof name == "string" && name.length > 0) {
        return `Function(${name})`;
      } else {
        return "Function";
      }
    }
    // objects
    if (Array.isArray(val)) {
      const length = val.length;
      let debug = "[";
      if (length > 0) {
        debug += debugString(val[0]);
      }
      for(let i = 1; i < length; i++){
        debug += ", " + debugString(val[i]);
      }
      debug += "]";
      return debug;
    }
    // Test for built-in
    const builtInMatches = /\[object ([^\]]+)\]/.exec(toString.call(val));
    let className;
    if (builtInMatches.length > 1) {
      className = builtInMatches[1];
    } else {
      // Failed to match the standard '[object ClassName]'
      return toString.call(val);
    }
    if (className == "Object") {
      // we're a user defined class or Object
      // JSON.stringify avoids problems with cycles, and is generally much
      // easier than looping through ownProperties of `val`.
      try {
        return "Object(" + JSON.stringify(val) + ")";
      } catch (_) {
        return "Object";
      }
    }
    // errors
    if (val instanceof Error) {
      return `${val.name}: ${val.message}\n${val.stack}`;
    }
    // TODO we could test for more things here, like `Set`s and `Map`s.
    return className;
  }
  const CLOSURE_DTORS = new FinalizationRegistry((state)=>{
    wasm.__wbindgen_export_2.get(state.dtor)(state.a, state.b);
  });
  function makeMutClosure(arg0, arg1, dtor, f) {
    const state = {
      a: arg0,
      b: arg1,
      cnt: 1,
      dtor
    };
    const real = (...args)=>{
      // First up with a closure we increment the internal reference
      // count. This ensures that the Rust closure environment won't
      // be deallocated while we're invoking it.
      state.cnt++;
      const a = state.a;
      state.a = 0;
      try {
        return f(a, state.b, ...args);
      } finally{
        if (--state.cnt === 0) {
          wasm.__wbindgen_export_2.get(state.dtor)(a, state.b);
          CLOSURE_DTORS.unregister(state);
        } else {
          state.a = a;
        }
      }
    };
    real.original = state;
    CLOSURE_DTORS.register(real, state, state);
    return real;
  }
  function __wbg_adapter_48(arg0, arg1, arg2) {
    wasm.wasm_bindgen__convert__closures__invoke1_mut__h989df39ca5fbddbf(arg0, arg1, addHeapObject(arg2));
  }
  function pack(options, on_diagnostic) {
    const ret = wasm.pack(addHeapObject(options), addHeapObject(on_diagnostic));
    return takeObject(ret);
  }
  function handleError(f, args) {
    try {
      return f.apply(this, args);
    } catch (e) {
      wasm.__wbindgen_exn_store(addHeapObject(e));
    }
  }
  function __wbg_adapter_94(arg0, arg1, arg2, arg3) {
    wasm.wasm_bindgen__convert__closures__invoke2_mut__h5af93f1c388db5f7(arg0, arg1, addHeapObject(arg2), addHeapObject(arg3));
  }
  const imports = {
    __wbindgen_placeholder__: {
      __wbindgen_is_undefined: function(arg0) {
        const ret = getObject(arg0) === undefined;
        return ret;
      },
      __wbindgen_in: function(arg0, arg1) {
        const ret = getObject(arg0) in getObject(arg1);
        return ret;
      },
      __wbindgen_number_get: function(arg0, arg1) {
        const obj = getObject(arg1);
        const ret = typeof obj === "number" ? obj : undefined;
        getFloat64Memory0()[arg0 / 8 + 1] = isLikeNone(ret) ? 0 : ret;
        getInt32Memory0()[arg0 / 4 + 0] = !isLikeNone(ret);
      },
      __wbindgen_boolean_get: function(arg0) {
        const v = getObject(arg0);
        const ret = typeof v === "boolean" ? v ? 1 : 0 : 2;
        return ret;
      },
      __wbindgen_string_get: function(arg0, arg1) {
        const obj = getObject(arg1);
        const ret = typeof obj === "string" ? obj : undefined;
        var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len1 = WASM_VECTOR_LEN;
        getInt32Memory0()[arg0 / 4 + 1] = len1;
        getInt32Memory0()[arg0 / 4 + 0] = ptr1;
      },
      __wbindgen_is_bigint: function(arg0) {
        const ret = typeof getObject(arg0) === "bigint";
        return ret;
      },
      __wbindgen_is_object: function(arg0) {
        const val = getObject(arg0);
        const ret = typeof val === "object" && val !== null;
        return ret;
      },
      __wbindgen_bigint_from_i64: function(arg0) {
        const ret = arg0;
        return addHeapObject(ret);
      },
      __wbindgen_bigint_from_u64: function(arg0) {
        const ret = BigInt.asUintN(64, arg0);
        return addHeapObject(ret);
      },
      __wbindgen_error_new: function(arg0, arg1) {
        const ret = new Error(getStringFromWasm0(arg0, arg1));
        return addHeapObject(ret);
      },
      __wbindgen_string_new: function(arg0, arg1) {
        const ret = getStringFromWasm0(arg0, arg1);
        return addHeapObject(ret);
      },
      __wbindgen_jsval_eq: function(arg0, arg1) {
        const ret = getObject(arg0) === getObject(arg1);
        return ret;
      },
      __wbindgen_object_drop_ref: function(arg0) {
        takeObject(arg0);
      },
      __wbg_fetchspecifier_f58f9309bbec04e6: function(arg0, arg1) {
        let deferred0_0;
        let deferred0_1;
        try {
          deferred0_0 = arg0;
          deferred0_1 = arg1;
          const ret = pack3.fetch_specifier(getStringFromWasm0(arg0, arg1));
          return addHeapObject(ret);
        } finally{
          wasm.__wbindgen_free(deferred0_0, deferred0_1);
        }
      },
      __wbindgen_is_null: function(arg0) {
        const ret = getObject(arg0) === null;
        return ret;
      },
      __wbg_new_abda76e883ba8a5f: function() {
        const ret = new Error();
        return addHeapObject(ret);
      },
      __wbg_stack_658279fe44541cf6: function(arg0, arg1) {
        const ret = getObject(arg1).stack;
        const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        getInt32Memory0()[arg0 / 4 + 1] = len1;
        getInt32Memory0()[arg0 / 4 + 0] = ptr1;
      },
      __wbg_error_f851667af71bcfc6: function(arg0, arg1) {
        let deferred0_0;
        let deferred0_1;
        try {
          deferred0_0 = arg0;
          deferred0_1 = arg1;
          console.error(getStringFromWasm0(arg0, arg1));
        } finally{
          wasm.__wbindgen_free(deferred0_0, deferred0_1);
        }
      },
      __wbindgen_object_clone_ref: function(arg0) {
        const ret = getObject(arg0);
        return addHeapObject(ret);
      },
      __wbindgen_jsval_loose_eq: function(arg0, arg1) {
        const ret = getObject(arg0) == getObject(arg1);
        return ret;
      },
      __wbindgen_number_new: function(arg0) {
        const ret = arg0;
        return addHeapObject(ret);
      },
      __wbg_getwithrefkey_5e6d9547403deab8: function(arg0, arg1) {
        const ret = getObject(arg0)[getObject(arg1)];
        return addHeapObject(ret);
      },
      __wbg_set_841ac57cff3d672b: function(arg0, arg1, arg2) {
        getObject(arg0)[takeObject(arg1)] = takeObject(arg2);
      },
      __wbindgen_cb_drop: function(arg0) {
        const obj = takeObject(arg0).original;
        if (obj.cnt-- == 1) {
          obj.a = 0;
          return true;
        }
        const ret = false;
        return ret;
      },
      __wbindgen_is_function: function(arg0) {
        const ret = typeof getObject(arg0) === "function";
        return ret;
      },
      __wbg_next_f4bc0e96ea67da68: function(arg0) {
        const ret = getObject(arg0).next;
        return addHeapObject(ret);
      },
      __wbg_value_2f4ef2036bfad28e: function(arg0) {
        const ret = getObject(arg0).value;
        return addHeapObject(ret);
      },
      __wbg_iterator_7c7e58f62eb84700: function() {
        const ret = Symbol.iterator;
        return addHeapObject(ret);
      },
      __wbg_new_2b6fea4ea03b1b95: function() {
        const ret = new Object();
        return addHeapObject(ret);
      },
      __wbg_get_7303ed2ef026b2f5: function(arg0, arg1) {
        const ret = getObject(arg0)[arg1 >>> 0];
        return addHeapObject(ret);
      },
      __wbg_isArray_04e59fb73f78ab5b: function(arg0) {
        const ret = Array.isArray(getObject(arg0));
        return ret;
      },
      __wbg_length_820c786973abdd8a: function(arg0) {
        const ret = getObject(arg0).length;
        return ret;
      },
      __wbg_instanceof_ArrayBuffer_ef2632aa0d4bfff8: function(arg0) {
        let result;
        try {
          result = getObject(arg0) instanceof ArrayBuffer;
        } catch  {
          result = false;
        }
        const ret = result;
        return ret;
      },
      __wbg_call_557a2f2deacc4912: function() {
        return handleError(function(arg0, arg1) {
          const ret = getObject(arg0).call(getObject(arg1));
          return addHeapObject(ret);
        }, arguments);
      },
      __wbg_call_587b30eea3e09332: function() {
        return handleError(function(arg0, arg1, arg2) {
          const ret = getObject(arg0).call(getObject(arg1), getObject(arg2));
          return addHeapObject(ret);
        }, arguments);
      },
      __wbg_next_ec061e48a0e72a96: function() {
        return handleError(function(arg0) {
          const ret = getObject(arg0).next();
          return addHeapObject(ret);
        }, arguments);
      },
      __wbg_done_b6abb27d42b63867: function(arg0) {
        const ret = getObject(arg0).done;
        return ret;
      },
      __wbg_isSafeInteger_2088b01008075470: function(arg0) {
        const ret = Number.isSafeInteger(getObject(arg0));
        return ret;
      },
      __wbg_entries_13e011453776468f: function(arg0) {
        const ret = Object.entries(getObject(arg0));
        return addHeapObject(ret);
      },
      __wbg_get_f53c921291c381bd: function() {
        return handleError(function(arg0, arg1) {
          const ret = Reflect.get(getObject(arg0), getObject(arg1));
          return addHeapObject(ret);
        }, arguments);
      },
      __wbg_buffer_55ba7a6b1b92e2ac: function(arg0) {
        const ret = getObject(arg0).buffer;
        return addHeapObject(ret);
      },
      __wbg_new_2b55e405e4af4986: function(arg0, arg1) {
        try {
          var state0 = {
            a: arg0,
            b: arg1
          };
          var cb0 = (arg0, arg1)=>{
            const a = state0.a;
            state0.a = 0;
            try {
              return __wbg_adapter_94(a, state0.b, arg0, arg1);
            } finally{
              state0.a = a;
            }
          };
          const ret = new Promise(cb0);
          return addHeapObject(ret);
        } finally{
          state0.a = state0.b = 0;
        }
      },
      __wbg_resolve_ae38ad63c43ff98b: function(arg0) {
        const ret = Promise.resolve(getObject(arg0));
        return addHeapObject(ret);
      },
      __wbg_then_8df675b8bb5d5e3c: function(arg0, arg1) {
        const ret = getObject(arg0).then(getObject(arg1));
        return addHeapObject(ret);
      },
      __wbg_then_835b073a479138e5: function(arg0, arg1, arg2) {
        const ret = getObject(arg0).then(getObject(arg1), getObject(arg2));
        return addHeapObject(ret);
      },
      __wbg_new_09938a7d020f049b: function(arg0) {
        const ret = new Uint8Array(getObject(arg0));
        return addHeapObject(ret);
      },
      __wbg_instanceof_Uint8Array_1349640af2da2e88: function(arg0) {
        let result;
        try {
          result = getObject(arg0) instanceof Uint8Array;
        } catch  {
          result = false;
        }
        const ret = result;
        return ret;
      },
      __wbg_length_0aab7ffd65ad19ed: function(arg0) {
        const ret = getObject(arg0).length;
        return ret;
      },
      __wbg_set_3698e3ca519b3c3c: function(arg0, arg1, arg2) {
        getObject(arg0).set(getObject(arg1), arg2 >>> 0);
      },
      __wbindgen_bigint_get_as_i64: function(arg0, arg1) {
        const v = getObject(arg1);
        const ret = typeof v === "bigint" ? v : undefined;
        getBigInt64Memory0()[arg0 / 8 + 1] = isLikeNone(ret) ? BigInt(0) : ret;
        getInt32Memory0()[arg0 / 4 + 0] = !isLikeNone(ret);
      },
      __wbindgen_debug_string: function(arg0, arg1) {
        const ret = debugString(getObject(arg1));
        const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        getInt32Memory0()[arg0 / 4 + 1] = len1;
        getInt32Memory0()[arg0 / 4 + 0] = ptr1;
      },
      __wbindgen_throw: function(arg0, arg1) {
        throw new Error(getStringFromWasm0(arg0, arg1));
      },
      __wbindgen_memory: function() {
        const ret = wasm.memory;
        return addHeapObject(ret);
      },
      __wbindgen_closure_wrapper2120: function(arg0, arg1, arg2) {
        const ret = makeMutClosure(arg0, arg1, 257, __wbg_adapter_48);
        return addHeapObject(ret);
      }
    }
  };
  const loader = new pack4.Loader({
    imports,
    cache: pack5.cacheToLocalDir
  });
  async function instantiate(opts) {
    return (await instantiateWithInstance(opts)).exports;
  }
  async function instantiateWithInstance(opts) {
    const { instance } = await loader.load(opts?.url ?? new URL("rs_lib_bg.wasm", import.meta.url), opts?.decompress);
    wasm = wasm ?? instance.exports;
    cachedInt32Memory0 = cachedInt32Memory0 ?? new Int32Array(wasm.memory.buffer);
    cachedUint8Memory0 = cachedUint8Memory0 ?? new Uint8Array(wasm.memory.buffer);
    return {
      instance,
      exports: getWasmInstanceExports()
    };
  }
  function getWasmInstanceExports() {
    return {
      pack
    };
  }
  function isInstantiated() {
    return loader.instance != null;
  }
  Object.defineProperty(pack0, "pack", {
    get: ()=>pack
  });
  Object.defineProperty(pack0, "instantiate", {
    get: ()=>instantiate
  });
  Object.defineProperty(pack0, "instantiateWithInstance", {
    get: ()=>instantiateWithInstance
  });
  Object.defineProperty(pack0, "isInstantiated", {
    get: ()=>isInstantiated
  });
})();
export function outputDiagnostic(diagnostic) {
  console.warn(`ERROR: ${diagnostic.message} -- ${diagnostic.specifier}${formatLineAndColumn(diagnostic.lineAndColumn)}`);
}
function formatLineAndColumn(lineAndColumn) {
  if (lineAndColumn == null) {
    return "";
  }
  return `:${lineAndColumn.lineNumber}:${lineAndColumn.columnNumber}`;
}
export async function pack(options) {
  const rs = await pack0.instantiate();
  const importMapUrl = options.importMap == null ? undefined : pack1.toFileUrl(pack1.resolve(options.importMap));
  let diagnosticCount = 0;
  const output = await rs.pack({
    entryPoints: [
      pack1.toFileUrl(pack1.resolve(options.entryPoint)).toString()
    ],
    importMap: importMapUrl?.toString()
  }, (diagnostic)=>{
    if (options.onDiagnostic) {
      options.onDiagnostic(diagnostic);
    } else {
      diagnosticCount++;
      outputDiagnostic(diagnostic);
    }
  });
  const baseNameNoExt = pack1.basename(options.entryPoint).slice(0, pack1.extname(options.entryPoint).length * -1);
  const jsOutputFolder = pack1.resolve(options.outputFolder);
  const jsOutputPath = pack1.join(options.outputFolder, `${baseNameNoExt}.js`);
  const tsOutputPath = pack1.join(options.outputFolder, `${baseNameNoExt}.ts`);
  const dtsOutputPath = pack1.join(jsOutputFolder, `${baseNameNoExt}.d.ts`);
  await Deno.mkdir(jsOutputFolder, {
    recursive: true
  });
  await Deno.writeTextFileSync(jsOutputPath, `/// <reference types="./${baseNameNoExt}.d.ts" />\n${output.js}`);
  await Deno.writeTextFileSync(tsOutputPath, (()=>{
    let text = `// @deno-types="./${baseNameNoExt}.d.ts"\nexport * from "./${baseNameNoExt}.js";\n`;
    if (output.hasDefaultExport) {
      text += `// @deno-types="./${baseNameNoExt}.d.ts"\n`;
      text += `import defaultExport from "./${baseNameNoExt}.js";\n`;
      text += `export default defaultExport;`;
    }
    return text;
  })());
  // todo: https://github.com/swc-project/swc/issues/7492
  await Deno.writeTextFileSync(dtsOutputPath, output.dts.replaceAll("*/ ", "*/\n"));
  if (diagnosticCount > 0) {
    throw new Error(`Failed. Had ${diagnosticCount} diagnostic${diagnosticCount != 1 ? "s" : ""}.`);
  }
  if ((options.typeCheck ?? true) && options.testFile == null) {
    const checkOutput = await new Deno.Command(Deno.execPath(), {
      args: [
        "check",
        "--no-config",
        tsOutputPath
      ]
    }).spawn();
    if (!await checkOutput.status) {
      Deno.exit(1);
    }
  }
  if (options.testFile != null) {
    const importMapObj = output.importMap == null ? {} : JSON.parse(output.importMap);
    importMapObj.imports ??= {};
    importMapObj.imports[pack1.toFileUrl(pack1.resolve(options.entryPoint)).toString()] = pack1.toFileUrl(tsOutputPath).toString();
    // todo: needs to handle scopes
    if (importMapUrl != null) {
      for (const [key, value] of Object.entries(importMapObj.imports)){
        if (value.startsWith("./")) {
          importMapObj.imports[key] = new URL(value, importMapUrl).toString();
        }
      }
    }
    const uri = `data:,${JSON.stringify(importMapObj)}`;
    // todo: configurable permissions
    const args = [
      "test",
      "-A",
      "--import-map",
      uri
    ];
    if (options.typeCheck === false) {
      args.push("--no-check");
    }
    args.push(options.testFile);
    const testOutput = await new Deno.Command(Deno.execPath(), {
      args
    }).spawn();
    if (!await testOutput.status) {
      Deno.exit(1);
    }
  }
}
