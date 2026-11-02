import {test,expect} from 'bun:test';
import {readConfig} from '../src/config';
import {validateDomainOwnership} from '../src/domain-ownership';
test('default installation owns core transports without enabling a product',()=>{
 const config=readConfig({});
 expect(config.domains).toEqual(['auth','portal','registry']);
 expect(Object.values(config.modules!.modules).every(state=>state==='disabled')).toBe(true);
 validateDomainOwnership(config.domains);
});
test('MCP can expose core and installed plugin tools without hosting or lifecycle',()=>{
 validateDomainOwnership(['auth','portal','registry','mcp']);
 expect(()=>validateDomainOwnership(['auth','mcp'])).toThrow('registry');
 expect(()=>validateDomainOwnership(['portal','registry'])).toThrow('Auth');
});
